//! Exact peer observation for a held local Unix stream. This does not authenticate
//! a process or filesystem socket path; the caller owns that identity bracket.
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(1);
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const SEQUENCE: u32 = 1;
const UNIX_DIAG_PEER: u16 = 2;
const UNIX_DIAG_SHUTDOWN: u16 = 6;
const UDIAG_SHOW_PEER: u32 = 4;
const UNAVAILABLE: &str = "Unix peer observation unavailable";
const INVALID: &str = "invalid Unix peer observation";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PeerIdentity {
    pub local_inode: u32,
    pub cookie: [u32; 2],
    pub peer_inode: u32,
}

/// Query only this stream's inode, never dump other sockets. Keep the stream open
/// while matching peer_inode to the authenticated relay's socket descriptors.
pub(crate) fn peer_identity(stream: &UnixStream) -> Result<PeerIdentity, String> {
    let deadline = Instant::now() + TIMEOUT;
    let local_inode = socket_inode(stream)?;
    // SAFETY: socket has no pointer arguments; OwnedFd takes the new descriptor.
    let raw = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            libc::NETLINK_SOCK_DIAG,
        )
    };
    if raw < 0 {
        return Err(UNAVAILABLE.into());
    }
    // SAFETY: raw is a newly allocated descriptor exclusively owned here.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    let kernel = netlink_address();
    let length = mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
    // Bind an automatic local port and connect only to kernel port zero.
    // SAFETY: kernel is initialized and its advertised length is exact.
    if unsafe { libc::bind(raw, (&kernel as *const libc::sockaddr_nl).cast(), length) } != 0
        || unsafe { libc::connect(raw, (&kernel as *const libc::sockaddr_nl).cast(), length) } != 0
    {
        return Err(UNAVAILABLE.into());
    }
    let mut local = netlink_address();
    let mut local_length = length;
    // SAFETY: both output pointers refer to initialized writable storage.
    if unsafe {
        libc::getsockname(
            raw,
            (&mut local as *mut libc::sockaddr_nl).cast(),
            &mut local_length,
        )
    } != 0
        || local_length != length
        || local.nl_family != libc::AF_NETLINK as libc::sa_family_t
        || local.nl_pid == 0
        || local.nl_groups != 0
    {
        return Err(UNAVAILABLE.into());
    }
    let request = request(local_inode, local.nl_pid);
    loop {
        wait(socket.as_raw_fd(), libc::POLLOUT, deadline)?;
        // SAFETY: request is a live fixed-size byte buffer, and socket is owned.
        let written = unsafe { libc::send(raw, request.as_ptr().cast(), request.len(), 0) };
        if written == request.len() as isize {
            break;
        }
        if written >= 0 || !retryable() {
            return Err(UNAVAILABLE.into());
        }
    }
    let mut bytes = [0_u8; MAX_REPLY];
    loop {
        wait(socket.as_raw_fd(), libc::POLLIN, deadline)?;
        let mut sender = netlink_address();
        let mut buffer = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        // SAFETY: msghdr has no invalid bit patterns; pointers are set below.
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_name = (&mut sender as *mut libc::sockaddr_nl).cast();
        message.msg_namelen = length;
        message.msg_iov = &mut buffer;
        message.msg_iovlen = 1;
        // No control buffer: unexpected ancillary data sets MSG_CTRUNC and fails.
        // SAFETY: all advertised buffers are live and correctly sized.
        let received = unsafe { libc::recvmsg(raw, &mut message, libc::MSG_DONTWAIT) };
        if received < 0 {
            if retryable() {
                continue;
            }
            return Err(UNAVAILABLE.into());
        }
        if Instant::now() >= deadline || received as usize > bytes.len() {
            return Err(UNAVAILABLE.into());
        }
        validate_sender(
            &sender,
            message.msg_namelen,
            message.msg_flags,
            message.msg_controllen,
        )?;
        return parse_reply(&bytes[..received as usize], local_inode, local.nl_pid);
    }
}

fn netlink_address() -> libc::sockaddr_nl {
    // SAFETY: all-zero is valid for sockaddr_nl, including its reserved padding.
    let mut address: libc::sockaddr_nl = unsafe { mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    address
}

fn socket_inode(stream: &UnixStream) -> Result<u32, String> {
    // SAFETY: stat has no invalid bit patterns and fstat initializes its fields.
    let mut metadata: libc::stat = unsafe { mem::zeroed() };
    // SAFETY: stream owns the borrowed descriptor and metadata is writable.
    if unsafe { libc::fstat(stream.as_raw_fd(), &mut metadata) } != 0
        || metadata.st_mode & libc::S_IFMT != libc::S_IFSOCK
    {
        return Err(UNAVAILABLE.into());
    }
    u32::try_from(metadata.st_ino)
        .ok()
        .filter(|inode| *inode != 0)
        .ok_or_else(|| UNAVAILABLE.into())
}

fn retryable() -> bool {
    matches!(
        io::Error::last_os_error().kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

fn wait(fd: RawFd, events: i16, deadline: Instant) -> Result<(), String> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(UNAVAILABLE.into());
        }
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let milliseconds = left.as_millis().clamp(1, 1000) as i32;
        // SAFETY: descriptor is one initialized writable pollfd.
        let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if result <= 0 || Instant::now() >= deadline || descriptor.revents != events {
            return Err(UNAVAILABLE.into());
        }
        return Ok(());
    }
}

fn request(inode: u32, port: u32) -> [u8; 40] {
    let mut bytes = [0_u8; 40];
    bytes[0..4].copy_from_slice(&40_u32.to_ne_bytes());
    bytes[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    bytes[6..8].copy_from_slice(&1_u16.to_ne_bytes()); // NLM_F_REQUEST, no DUMP
    bytes[8..12].copy_from_slice(&SEQUENCE.to_ne_bytes());
    bytes[12..16].copy_from_slice(&port.to_ne_bytes());
    bytes[16] = libc::AF_UNIX as u8;
    bytes[20..24].copy_from_slice(&u32::MAX.to_ne_bytes()); // all states, exact inode
    bytes[24..28].copy_from_slice(&inode.to_ne_bytes());
    bytes[28..32].copy_from_slice(&UDIAG_SHOW_PEER.to_ne_bytes());
    bytes[32..40].fill(0xff); // UDIAG_NOCOOKIE; the held stream pins its inode.
    bytes
}

fn validate_sender(
    sender: &libc::sockaddr_nl,
    length: libc::socklen_t,
    flags: i32,
    control_length: usize,
) -> Result<(), String> {
    if length as usize != mem::size_of::<libc::sockaddr_nl>()
        || sender.nl_family != libc::AF_NETLINK as libc::sa_family_t
        || sender.nl_pid != 0
        || sender.nl_groups != 0
        || flags != 0
        || control_length != 0
    {
        return Err(INVALID.into());
    }
    Ok(())
}

fn parse_reply(bytes: &[u8], inode: u32, port: u32) -> Result<PeerIdentity, String> {
    if !(32..=MAX_REPLY).contains(&bytes.len()) {
        return Err(INVALID.into());
    }
    // Fixed offsets are read only after checking the whole fixed header.
    let u32_at = |offset| u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let u16_at = |offset| u16::from_ne_bytes(bytes[offset..offset + 2].try_into().unwrap());
    if u32_at(0) as usize != bytes.len()
        || u16_at(4) != SOCK_DIAG_BY_FAMILY
        || u16_at(6) != 0
        || u32_at(8) != SEQUENCE
        || u32_at(12) != port
        || bytes[16] != libc::AF_UNIX as u8
        || bytes[17] != libc::SOCK_STREAM as u8
        || bytes[18] != 1 // Linux TCP_ESTABLISHED, also used by Unix stream diag.
        || bytes[19] != 0
        || inode == 0
        || u32_at(20) != inode
    {
        return Err(INVALID.into());
    }
    let cookie = [u32_at(24), u32_at(28)];
    if cookie == [0; 2] || cookie == [u32::MAX; 2] {
        return Err(INVALID.into());
    }
    let mut peer = None;
    let mut shutdown = false;
    let mut offset = 32;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(INVALID.into());
        }
        let length = u16_at(offset) as usize;
        let kind = u16_at(offset + 2);
        let aligned = (length + 3) & !3;
        if length < 4
            || aligned > bytes.len() - offset
            || bytes[offset + length..offset + aligned]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(INVALID.into());
        }
        match kind {
            UNIX_DIAG_PEER if length == 8 && peer.is_none() => {
                let value = u32_at(offset + 4);
                if value == 0 {
                    return Err(INVALID.into());
                }
                peer = Some(value);
            }
            UNIX_DIAG_SHUTDOWN if length == 5 && !shutdown && bytes[offset + 4] == 0 => {
                shutdown = true;
            }
            _ => return Err(INVALID.into()),
        }
        offset += aligned;
    }
    if !shutdown {
        return Err(INVALID.into());
    }
    Ok(PeerIdentity {
        local_inode: inode,
        cookie,
        peer_inode: peer.ok_or(INVALID)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn observed_reply() -> Vec<u8> {
        // Captured Linux shape from the own-socket experiment: one peer u32 and
        // an unsolicited shutdown byte, with zero alignment padding.
        let mut bytes = vec![0; 48];
        bytes[0..4].copy_from_slice(&48_u32.to_ne_bytes());
        bytes[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        bytes[8..12].copy_from_slice(&SEQUENCE.to_ne_bytes());
        bytes[12..16].copy_from_slice(&99_u32.to_ne_bytes());
        bytes[16..20].copy_from_slice(&[1, 1, 1, 0]);
        bytes[20..24].copy_from_slice(&41_u32.to_ne_bytes());
        bytes[24..28].copy_from_slice(&8193_u32.to_ne_bytes());
        bytes[32..34].copy_from_slice(&8_u16.to_ne_bytes());
        bytes[34..36].copy_from_slice(&UNIX_DIAG_PEER.to_ne_bytes());
        bytes[36..40].copy_from_slice(&42_u32.to_ne_bytes());
        bytes[40..42].copy_from_slice(&5_u16.to_ne_bytes());
        bytes[42..44].copy_from_slice(&UNIX_DIAG_SHUTDOWN.to_ne_bytes());
        bytes
    }

    #[test]
    fn exact_query_and_observed_reply_have_closed_shapes() {
        let query = request(41, 99);
        assert_eq!(&query[6..8], &1_u16.to_ne_bytes());
        assert_eq!(&query[24..28], &41_u32.to_ne_bytes());
        assert_eq!(&query[28..32], &4_u32.to_ne_bytes());
        assert_eq!(&query[32..], &[0xff; 8]);
        assert_eq!(
            parse_reply(&observed_reply(), 41, 99).unwrap(),
            PeerIdentity {
                local_inode: 41,
                cookie: [8193, 0],
                peer_inode: 42,
            }
        );
    }

    #[test]
    fn replies_refuse_identity_protocol_flags_and_attribute_substitution() {
        let base = observed_reply();
        for (offset, value) in [
            (16, 2),
            (17, 2),
            (18, 0),
            (19, 1), // family/type/state/pad
            (44, 3),
            (45, 1), // shutdown value/padding
        ] {
            let mut changed = base.clone();
            changed[offset] = value;
            assert!(parse_reply(&changed, 41, 99).is_err(), "offset {offset}");
        }
        for (offset, value) in [(0, 47_u32), (8, 2), (12, 98), (20, 40), (36, 0)] {
            let mut changed = base.clone();
            changed[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            assert!(
                parse_reply(&changed, 41, 99).is_err(),
                "u32 offset {offset}"
            );
        }
        for (offset, value) in [(4, 2_u16), (6, 2), (32, 7), (34, 1), (40, 4), (42, 3)] {
            let mut changed = base.clone();
            changed[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
            assert!(
                parse_reply(&changed, 41, 99).is_err(),
                "u16 offset {offset}"
            );
        }
        for fill in [0, 0xff] {
            let mut cookie = base.clone();
            cookie[24..32].fill(fill);
            assert!(parse_reply(&cookie, 41, 99).is_err());
        }
        let mut flagged_attribute = base.clone();
        flagged_attribute[34..36].copy_from_slice(&(UNIX_DIAG_PEER | 0x8000).to_ne_bytes());
        assert!(parse_reply(&flagged_attribute, 41, 99).is_err());
        assert!(parse_reply(&base, 42, 99).is_err());
        assert!(parse_reply(&base, 41, 98).is_err());
    }

    #[test]
    fn replies_refuse_truncation_missing_duplicate_and_extra_messages() {
        let base = observed_reply();
        for length in 0..base.len() {
            let mut truncated = base[..length].to_vec();
            if length >= 4 {
                truncated[..4].copy_from_slice(&(length as u32).to_ne_bytes());
            }
            assert!(parse_reply(&truncated, 41, 99).is_err(), "length {length}");
        }
        for extra in [&base[32..40], &base[40..48], &base[..]] {
            let mut duplicate = base.clone();
            duplicate.extend_from_slice(extra);
            let length = duplicate.len() as u32;
            duplicate[..4].copy_from_slice(&length.to_ne_bytes());
            assert!(parse_reply(&duplicate, 41, 99).is_err());
        }
        assert!(parse_reply(&vec![0; MAX_REPLY + 1], 41, 99).is_err());
    }

    #[test]
    fn only_untruncated_kernel_unicast_datagrams_are_accepted() {
        let address = netlink_address();
        let length = mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        assert!(validate_sender(&address, length, 0, 0).is_ok());
        for flags in [libc::MSG_TRUNC, libc::MSG_CTRUNC, libc::MSG_OOB] {
            assert!(validate_sender(&address, length, flags, 0).is_err());
        }
        assert!(validate_sender(&address, length - 1, 0, 0).is_err());
        assert!(validate_sender(&address, length, 0, 1).is_err());
        let mut foreign = netlink_address();
        foreign.nl_pid = 123;
        assert!(validate_sender(&foreign, length, 0, 0).is_err());
        foreign = netlink_address();
        foreign.nl_groups = 1;
        assert!(validate_sender(&foreign, length, 0, 0).is_err());
        foreign = netlink_address();
        foreign.nl_family = libc::AF_UNIX as libc::sa_family_t;
        assert!(validate_sender(&foreign, length, 0, 0).is_err());
    }

    #[test]
    fn real_socketpair_identity_survives_requery_but_not_peer_close() {
        let (client, accepted) = UnixStream::pair().unwrap();
        let observed = peer_identity(&client).unwrap();
        assert_eq!(observed.local_inode, socket_inode(&client).unwrap());
        assert_eq!(observed.peer_inode, socket_inode(&accepted).unwrap());
        assert_eq!(peer_identity(&client).unwrap(), observed);
        let (other, other_peer) = UnixStream::pair().unwrap();
        let changed = peer_identity(&other).unwrap();
        assert_ne!(observed, changed);
        assert_eq!(changed.peer_inode, socket_inode(&other_peer).unwrap());
        drop(accepted);
        assert!(peer_identity(&client).is_err());
    }

    #[test]
    fn real_listener_peer_is_the_accepted_socket_not_the_path_inode() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Keep Unix addresses short even when the package builder's TMPDIR is long.
        let directory = std::path::Path::new("/tmp").join(format!(
            "tf-unix-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = RemoveDirectory(directory.clone());
        let path = directory.join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let observed = peer_identity(&client).unwrap();
        assert_eq!(observed.peer_inode, socket_inode(&accepted).unwrap());
        // The path lives in another inode namespace; only the accepted socket's
        // fstat value above establishes the peer identity (numbers may coincide).
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_socket());
        accepted.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(peer_identity(&client).is_err());
    }
}
