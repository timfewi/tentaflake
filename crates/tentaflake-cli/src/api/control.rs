//! Fixed local transport. Peer identity permits IPC, never a host action.
//! Frames may contain credentials; neither frames nor decode errors are logged.
use super::{MAX_BODY, Reply};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, OsStr};
use std::fs::File;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tentaflake_admission::protected_path::{open_at, open_root, protected_directory};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{Instant, timeout, timeout_at};

const SOCKET: &str = "/run/tentaflake-api-owner.sock";
const FRAME_LIMIT: usize = 512 * 1024;
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_LIMIT: usize = 32 * 1024;
type Headers = Vec<(String, Vec<u8>)>;
static ACCOUNT: OnceLock<Result<(u32, u32), String>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RequestFrame {
    schema_version: u8,
    method: String,
    target: String,
    headers: Headers,
    body: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReplyFrame {
    schema_version: u8,
    status: u16,
    headers: Headers,
    body: Vec<u8>,
}

fn header_bounds(values: &Headers) -> Result<(), ()> {
    if values.len() > 64
        || values
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
            > HEADER_LIMIT
    {
        return Err(());
    }
    Ok(())
}

fn headers(values: Headers) -> Result<HeaderMap, ()> {
    header_bounds(&values)?;
    let mut result = HeaderMap::new();
    for (name, value) in values {
        result.append(
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?,
            HeaderValue::from_bytes(&value).map_err(|_| ())?,
        );
    }
    Ok(result)
}

fn encode_headers(values: &HeaderMap) -> Result<Headers, ()> {
    let values: Headers = values
        .iter()
        .map(|(name, value)| (name.as_str().into(), value.as_bytes().to_vec()))
        .collect();
    header_bounds(&values)?;
    Ok(values)
}

impl RequestFrame {
    fn decode(self) -> Result<(hyper::http::request::Parts, Bytes), ()> {
        if self.schema_version != 1
            || self.method.is_empty()
            || self.method.len() > 32
            || self.target.is_empty()
            || self.target.len() > HEADER_LIMIT
            || self.body.len() > MAX_BODY
        {
            return Err(());
        }
        let mut request = Request::new(());
        *request.method_mut() = Method::from_bytes(self.method.as_bytes()).map_err(|_| ())?;
        *request.uri_mut() = self.target.parse::<Uri>().map_err(|_| ())?;
        *request.headers_mut() = headers(self.headers)?;
        // Absolute targets and queries survive transport for the owner's exact
        // refusal. No normalization, routing or authentication occurs here.
        Ok((request.into_parts().0, Bytes::from(self.body)))
    }
}

async fn read_frame<T: serde::de::DeserializeOwned>(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<T, ()> {
    timeout_at(deadline, async {
        let length = stream.read_u32().await.map_err(|_| ())? as usize;
        if length == 0 || length > FRAME_LIMIT {
            return Err(());
        }
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await.map_err(|_| ())?;
        serde_json::from_slice(&bytes).map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
}

async fn write_frame<T: Serialize>(stream: &mut UnixStream, frame: &T) -> Result<(), ()> {
    let bytes = serde_json::to_vec(frame).map_err(|_| ())?;
    if bytes.is_empty() || bytes.len() > FRAME_LIMIT {
        return Err(());
    }
    timeout(FRAME_TIMEOUT, async {
        stream.write_u32(bytes.len() as u32).await.map_err(|_| ())?;
        stream.write_all(&bytes).await.map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
}

pub(super) async fn read_request(
    stream: &mut UnixStream,
) -> Result<(hyper::http::request::Parts, Bytes), ()> {
    read_frame::<RequestFrame>(stream, Instant::now() + FRAME_TIMEOUT)
        .await?
        .decode()
}

pub(super) async fn write_reply(stream: &mut UnixStream, reply: Reply) -> Result<(), ()> {
    let (parts, body) = reply.into_parts();
    let body = body.collect().await.map_err(|_| ())?.to_bytes();
    if body.len() > MAX_BODY || !(200..600).contains(&parts.status.as_u16()) {
        return Err(());
    }
    write_frame(
        stream,
        &ReplyFrame {
            schema_version: 1,
            status: parts.status.as_u16(),
            headers: encode_headers(&parts.headers)?,
            body: body.to_vec(),
        },
    )
    .await
}

async fn exchange(
    mut stream: UnixStream,
    parts: hyper::http::request::Parts,
    body: Bytes,
) -> Result<Reply, ()> {
    timeout(Duration::from_secs(10), async {
        if body.len() > MAX_BODY {
            return Err(());
        }
        let frame = RequestFrame {
            schema_version: 1,
            method: parts.method.to_string(),
            target: parts.uri.to_string(),
            headers: encode_headers(&parts.headers)?,
            body: body.to_vec(),
        };
        if frame.body.len() > MAX_BODY
            || frame.method.len() > 32
            || frame.target.len() > HEADER_LIMIT
        {
            return Err(());
        }
        write_frame(&mut stream, &frame).await?;
        let frame: ReplyFrame = read_frame(&mut stream, Instant::now() + FRAME_TIMEOUT).await?;
        if frame.schema_version != 1
            || frame.body.len() > MAX_BODY
            || !(200..600).contains(&frame.status)
        {
            return Err(());
        }
        let mut reply = Response::new(Full::new(Bytes::from(frame.body)));
        *reply.status_mut() = StatusCode::from_u16(frame.status).map_err(|_| ())?;
        *reply.headers_mut() = headers(frame.headers)?;
        Ok(reply)
    })
    .await
    .map_err(|_| ())?
}

pub(super) fn check_peer(stream: &UnixStream, expected_uid: u32) -> Result<(), ()> {
    let peer = stream.peer_cred().map_err(|_| ())?;
    if peer.uid() != expected_uid || peer.pid().is_none_or(|pid| pid <= 0) {
        return Err(());
    }
    Ok(())
}

fn socket_at(directory: &File, name: &OsStr, owner: u32, group: u32) -> Result<File, ()> {
    protected_directory(directory, owner).map_err(|_| ())?;
    let socket = open_at(directory.as_raw_fd(), name, libc::O_PATH).map_err(|_| ())?;
    let metadata = socket.metadata().map_err(|_| ())?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner
        || metadata.gid() != group
        || metadata.mode() & 0o7777 != 0o660
        || metadata.nlink() != 1
    {
        return Err(());
    }
    Ok(socket)
}

fn socket_path(group: u32) -> Result<File, ()> {
    let directory = open_root(Path::new("/run"), 0).map_err(|_| ())?;
    socket_at(
        &directory,
        OsStr::new("tentaflake-api-owner.sock"),
        0,
        group,
    )
}

fn same_socket(left: &File, right: &File) -> Result<(), ()> {
    let left = left.metadata().map_err(|_| ())?;
    let right = right.metadata().map_err(|_| ())?;
    if (left.dev(), left.ino()) != (right.dev(), right.ino()) {
        return Err(());
    }
    Ok(())
}

fn lookup_account() -> Result<(u32, u32), String> {
    let mut user = MaybeUninit::<libc::passwd>::uninit();
    let mut group = MaybeUninit::<libc::group>::uninit();
    let mut user_result = std::ptr::null_mut();
    let mut group_result = std::ptr::null_mut();
    let mut user_buffer = [0_u8; 16 * 1024];
    let mut group_buffer = [0_u8; 16 * 1024];
    // SAFETY: both reentrant NSS calls receive live output structs and bounded
    // buffers. Success plus exact result-pointer equality precedes initialization
    // and C-string reads, whose storage remains alive through this block.
    unsafe {
        if libc::getpwnam_r(
            c"tentaflake-api".as_ptr(),
            user.as_mut_ptr(),
            user_buffer.as_mut_ptr().cast(),
            user_buffer.len(),
            &mut user_result,
        ) != 0
            || user_result != user.as_mut_ptr()
            || libc::getgrnam_r(
                c"tentaflake-api".as_ptr(),
                group.as_mut_ptr(),
                group_buffer.as_mut_ptr().cast(),
                group_buffer.len(),
                &mut group_result,
            ) != 0
            || group_result != group.as_mut_ptr()
        {
            return Err("dedicated host API account unavailable".into());
        }
        let user = user.assume_init();
        let group = group.assume_init();
        if user.pw_uid == 0
            || group.gr_gid == 0
            || user.pw_gid != group.gr_gid
            || user.pw_name.is_null()
            || group.gr_name.is_null()
            || CStr::from_ptr(user.pw_name) != c"tentaflake-api"
            || CStr::from_ptr(group.gr_name) != c"tentaflake-api"
        {
            return Err("invalid dedicated host API account".into());
        }
        Ok((user.pw_uid, group.gr_gid))
    }
}

fn account() -> Result<(u32, u32), String> {
    ACCOUNT.get_or_init(lookup_account).clone()
}

pub(super) fn frontend_uid() -> Result<u32, String> {
    Ok(account()?.0)
}

fn socket_option(fd: std::os::fd::RawFd, option: libc::c_int) -> Result<libc::c_int, ()> {
    let mut value: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&value) as libc::socklen_t;
    // SAFETY: value/length are writable, correctly sized outputs. getsockopt
    // validates the borrowed descriptor; no ownership is taken or pointer retained.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&mut value as *mut libc::c_int).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&value)
    {
        return Err(());
    }
    Ok(value)
}

pub(super) async fn call(parts: hyper::http::request::Parts, body: Bytes) -> Result<Reply, ()> {
    timeout(Duration::from_secs(10), async {
        // frontend_uid() authenticates the fixed account at service startup.
        // Never invoke potentially blocking NSS inside a request deadline.
        let (_, group) = ACCOUNT
            .get()
            .and_then(|value| value.as_ref().ok())
            .ok_or(())?;
        let before = socket_path(*group)?;
        let stream = UnixStream::connect(SOCKET).await.map_err(|_| ())?;
        // A systemd-created listening socket may report PID 1, not the worker.
        check_peer(&stream, 0)?;
        same_socket(&before, &socket_path(*group)?)?;
        exchange(stream, parts, body).await
    })
    .await
    .map_err(|_| ())?
}

pub(super) fn take_listener() -> Result<UnixListener, String> {
    let pid = std::process::id().to_string();
    if std::env::var("LISTEN_PID").ok().as_deref() != Some(pid.as_str())
        || std::env::var("LISTEN_FDS").ok().as_deref() != Some("1")
    {
        return Err("host API owner requires one systemd socket".into());
    }
    let (_, group) = account()?;
    let before = socket_path(group).map_err(|_| "invalid owner socket path")?;
    let fd = 3;
    // SAFETY: F_GETFD takes no pointer and validates the systemd descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    // SAFETY: F_SETFD takes an integer flag mask, not a pointer. Ownership is
    // still unique to startup; every error returns before adopting the descriptor.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err("invalid inherited owner socket".into());
    }
    if socket_option(fd, libc::SO_ACCEPTCONN) != Ok(1)
        || socket_option(fd, libc::SO_TYPE) != Ok(libc::SOCK_STREAM)
        || socket_option(fd, libc::SO_DOMAIN) != Ok(libc::AF_UNIX)
    {
        return Err("inherited owner socket is not listening".into());
    }
    // SAFETY: validated systemd activation transfers its sole listening FD to
    // this one startup call. It has not been wrapped/closed elsewhere, and the
    // returned listener now owns and closes it, including on later validation errors.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    if listener
        .local_addr()
        .ok()
        .and_then(|address| address.as_pathname().map(Path::to_path_buf))
        .as_deref()
        != Some(Path::new(SOCKET))
    {
        return Err("inherited owner socket has a different address".into());
    }
    same_socket(
        &before,
        &socket_path(group).map_err(|_| "owner socket changed")?,
    )
    .map_err(|_| "owner socket changed")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "cannot configure owner socket")?;
    Ok(listener)
}

#[cfg(test)]
pub(super) async fn exchange_for_test(
    stream: UnixStream,
    parts: hyper::http::request::Parts,
    body: Bytes,
) -> Result<Reply, ()> {
    exchange(stream, parts, body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, DirBuilder};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn valid() -> serde_json::Value {
        serde_json::json!({"schemaVersion":1,"method":"GET","target":"/api/v1/session",
            "headers":[["host",[104,111,115,116]]],"body":[]})
    }

    #[test]
    fn real_stream_roundtrip_preserves_auth_duplicates_raw_bytes_and_target() {
        runtime().block_on(async {
            let (client, mut owner) = UnixStream::pair().unwrap();
            let task = tokio::spawn(async move {
                let (parts, body) = read_request(&mut owner).await.unwrap();
                assert_eq!(parts.uri, "https://example.invalid/api/v1/session?query=1");
                assert_eq!(parts.headers.get_all("authorization").iter().count(), 2);
                assert_eq!(parts.headers["x-raw"].as_bytes(), &[0xff]);
                assert_eq!(parts.headers["upgrade"], "websocket");
                assert_eq!(body, Bytes::from_static(b"synthetic-body"));
                let mut reply = Response::new(Full::new(Bytes::from_static(b"{}")));
                reply
                    .headers_mut()
                    .append("set-cookie", HeaderValue::from_static("first=synthetic"));
                reply
                    .headers_mut()
                    .append("set-cookie", HeaderValue::from_static("second=synthetic"));
                write_reply(&mut owner, reply).await.unwrap();
            });
            let mut request = Request::builder()
                .method("POST")
                .uri("https://example.invalid/api/v1/session?query=1")
                .body(())
                .unwrap();
            for value in ["Bearer synthetic-one", "Bearer synthetic-two"] {
                request
                    .headers_mut()
                    .append("authorization", HeaderValue::from_static(value));
            }
            request
                .headers_mut()
                .insert("x-raw", HeaderValue::from_bytes(&[0xff]).unwrap());
            request
                .headers_mut()
                .insert("upgrade", HeaderValue::from_static("websocket"));
            let reply = exchange_for_test(
                client,
                request.into_parts().0,
                Bytes::from_static(b"synthetic-body"),
            )
            .await
            .unwrap();
            assert_eq!(reply.headers().get_all("set-cookie").iter().count(), 2);
            assert_eq!(
                reply.into_body().collect().await.unwrap().to_bytes(),
                Bytes::from_static(b"{}")
            );
            task.await.unwrap();
        });
    }

    #[test]
    fn closed_frames_and_semantic_bounds_refuse_malformed_input() {
        runtime().block_on(async {
            let mut invalid = Vec::new();
            for (field, value) in [
                ("schemaVersion", serde_json::json!(2)),
                ("authority", serde_json::json!("forged")),
                ("method", serde_json::json!("GET\r\n")),
                ("target", serde_json::json!("bad target")),
                ("headers", serde_json::json!([["bad header", []]])),
                ("headers", serde_json::json!([["host", [13, 10]]])),
                ("headers", serde_json::json!(vec![("host", vec![b'a']); 65])),
                (
                    "headers",
                    serde_json::json!([["host", vec![b'a'; HEADER_LIMIT]]]),
                ),
                ("body", serde_json::json!(vec![0; MAX_BODY + 1])),
            ] {
                let mut frame = valid();
                frame[field] = value;
                invalid.push(serde_json::to_vec(&frame).unwrap());
            }
            invalid.push(br#"{"schemaVersion":1,"schemaVersion":1}"#.to_vec());
            for bytes in invalid {
                let (mut sender, mut receiver) = UnixStream::pair().unwrap();
                let writer = tokio::spawn(async move {
                    sender.write_u32(bytes.len() as u32).await.unwrap();
                    sender.write_all(&bytes).await.unwrap();
                });
                assert!(read_request(&mut receiver).await.is_err());
                writer.await.unwrap();
            }
        });
    }

    #[test]
    fn frame_prefix_rejects_without_payload_and_truncated_payload_refuses() {
        runtime().block_on(async {
            for length in [0, FRAME_LIMIT as u32 + 1, u32::MAX] {
                let (mut sender, mut receiver) = UnixStream::pair().unwrap();
                sender.write_u32(length).await.unwrap();
                assert!(
                    timeout(Duration::from_millis(100), read_request(&mut receiver))
                        .await
                        .unwrap()
                        .is_err()
                );
            }
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            sender.write_u32(32).await.unwrap();
            sender.write_all(b"{").await.unwrap();
            drop(sender);
            assert!(read_request(&mut receiver).await.is_err());
        });
    }

    #[test]
    fn one_deadline_covers_partial_frame_and_cancellation_drops_transport() {
        runtime().block_on(async {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            sender.write_u32(32).await.unwrap();
            sender.write_all(b"{").await.unwrap();
            assert!(
                read_frame::<RequestFrame>(
                    &mut receiver,
                    Instant::now() + Duration::from_millis(20)
                )
                .await
                .is_err()
            );
            drop(receiver);
            let mut byte = [0];
            assert_eq!(sender.read(&mut byte).await.unwrap(), 0);

            let (client, mut owner) = UnixStream::pair().unwrap();
            let task = tokio::spawn(exchange_for_test(
                client,
                Request::new(()).into_parts().0,
                Bytes::new(),
            ));
            read_request(&mut owner).await.unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(owner.read(&mut byte).await.unwrap(), 0);
        });
    }

    #[test]
    fn peer_credentials_and_reply_limits_are_enforced() {
        runtime().block_on(async {
            let (mut peer, other) = UnixStream::pair().unwrap();
            let uid = tentaflake_admission::effective_uid();
            assert!(check_peer(&peer, uid).is_ok());
            assert!(check_peer(&peer, uid.wrapping_add(1)).is_err());
            assert!(
                write_reply(
                    &mut peer,
                    Response::new(Full::new(Bytes::from(vec![0; MAX_BODY + 1])))
                )
                .await
                .is_err()
            );
            assert!(socket_option(other.as_raw_fd(), libc::SO_ACCEPTCONN) == Ok(0));
        });
    }

    #[test]
    fn client_refuses_closed_reply_schema_and_invalid_http_values() {
        runtime().block_on(async {
            for (field, value) in [
                ("schemaVersion", serde_json::json!(2)),
                ("private", serde_json::json!("unexpected")),
                ("status", serde_json::json!(101)),
                ("headers", serde_json::json!([["set-cookie", [13, 10]]])),
                ("body", serde_json::json!(vec![0; MAX_BODY + 1])),
            ] {
                let (client, mut owner) = UnixStream::pair().unwrap();
                let mut frame =
                    serde_json::json!({"schemaVersion":1,"status":200,"headers":[],"body":[]});
                frame[field] = value;
                let task = tokio::spawn(async move {
                    read_request(&mut owner).await.unwrap();
                    write_frame(&mut owner, &frame).await.unwrap();
                });
                assert!(
                    exchange_for_test(client, Request::new(()).into_parts().0, Bytes::new())
                        .await
                        .is_err()
                );
                task.await.unwrap();
            }
        });
    }

    #[test]
    fn held_socket_metadata_refuses_aliases_permissions_and_replacement() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tentaflake-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        let directory = File::open(&path).unwrap();
        let socket = path.join("owner.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
        let uid = tentaflake_admission::effective_uid();
        let gid = tentaflake_admission::effective_gid();
        let held = socket_at(&directory, OsStr::new("owner.sock"), uid, gid).unwrap();
        assert_eq!(
            socket_option(listener.as_raw_fd(), libc::SO_TYPE),
            Ok(libc::SOCK_STREAM)
        );
        assert_eq!(
            socket_option(listener.as_raw_fd(), libc::SO_ACCEPTCONN),
            Ok(1)
        );
        assert!(
            socket_at(
                &directory,
                OsStr::new("owner.sock"),
                uid.wrapping_add(1),
                gid
            )
            .is_err()
        );
        assert!(
            socket_at(
                &directory,
                OsStr::new("owner.sock"),
                uid,
                gid.wrapping_add(1)
            )
            .is_err()
        );
        fs::write(path.join("regular"), b"not a socket").unwrap();
        fs::set_permissions(path.join("regular"), fs::Permissions::from_mode(0o660)).unwrap();
        assert!(socket_at(&directory, OsStr::new("regular"), uid, gid).is_err());
        symlink("owner.sock", path.join("alias")).unwrap();
        assert!(socket_at(&directory, OsStr::new("alias"), uid, gid).is_err());
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(socket_at(&directory, OsStr::new("owner.sock"), uid, gid).is_err());
        fs::remove_file(&socket).unwrap();
        let replacement = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
        assert!(
            same_socket(
                &held,
                &socket_at(&directory, OsStr::new("owner.sock"), uid, gid).unwrap()
            )
            .is_err()
        );
        drop((listener, replacement, held, directory));
        fs::remove_dir_all(path).unwrap();
    }
}
