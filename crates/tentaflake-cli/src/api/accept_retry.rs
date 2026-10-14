//! Recoverable listener failures must not revoke the owner's live sessions.
use std::future::Future;
use std::io;
use std::time::Duration;

const BACKOFF: Duration = Duration::from_millis(100);

pub(super) async fn accept<T, F, Fut>(mut accept_once: F) -> io::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    loop {
        match accept_once().await {
            Ok(connection) => return Ok(connection),
            Err(error) if transient(&error) => tokio::time::sleep(BACKOFF).await,
            Err(error) => return Err(error),
        }
    }
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
    ) || matches!(
        error.raw_os_error(),
        Some(
            libc::EMFILE
                | libc::ENFILE
                | libc::ENOBUFS
                | libc::ENOMEM
                | libc::EPROTO
                | libc::ENETDOWN
                | libc::ENOPROTOOPT
                | libc::EHOSTDOWN
                | libc::ENONET
                | libc::EHOSTUNREACH
                | libc::EOPNOTSUPP
                | libc::ENETUNREACH
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn transient_errors_back_off_and_resume_but_bad_descriptors_fail() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let errors = [
                    libc::EMFILE,
                    libc::ENFILE,
                    libc::ECONNABORTED,
                    libc::ENOBUFS,
                    libc::ENOMEM,
                    libc::EINTR,
                    libc::EAGAIN,
                    libc::EPROTO,
                ];
                let mut pending = errors.into_iter();
                let started = std::time::Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(10),
                    accept(|| {
                        std::future::ready(match pending.next() {
                            Some(error) => Err(io::Error::from_raw_os_error(error)),
                            None => Ok("accepted"),
                        })
                    }),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(result, "accepted");
                assert!(started.elapsed() >= BACKOFF * errors.len() as u32);
                let calls = Cell::new(0);
                let failure = accept(|| {
                    calls.set(calls.get() + 1);
                    std::future::ready(Err::<(), _>(io::Error::from_raw_os_error(libc::EBADF)))
                })
                .await
                .unwrap_err();
                assert_eq!(failure.raw_os_error(), Some(libc::EBADF));
                assert_eq!(calls.get(), 1);
            });
    }

    #[test]
    fn tcp_and_unix_listeners_accept_after_transient_failures() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let client = tokio::net::TcpStream::connect(tcp.local_addr().unwrap())
                    .await
                    .unwrap();
                let fail = Cell::new(true);
                let (accepted, peer) = accept(|| async {
                    if fail.replace(false) {
                        Err(io::Error::from_raw_os_error(libc::EMFILE))
                    } else {
                        tcp.accept().await
                    }
                })
                .await
                .unwrap();
                assert_eq!(peer, client.local_addr().unwrap());
                drop((accepted, client));

                static SEQUENCE: AtomicU64 = AtomicU64::new(0);
                let path = std::env::temp_dir().join(format!(
                    "tentaflake-accept-{}-{}.sock",
                    std::process::id(),
                    SEQUENCE.fetch_add(1, Ordering::Relaxed),
                ));
                let unix = tokio::net::UnixListener::bind(&path).unwrap();
                let client = tokio::net::UnixStream::connect(&path).await.unwrap();
                let fail = Cell::new(true);
                let (accepted, _) = accept(|| async {
                    if fail.replace(false) {
                        Err(io::Error::from_raw_os_error(libc::ENOBUFS))
                    } else {
                        unix.accept().await
                    }
                })
                .await
                .unwrap();
                assert_eq!(
                    accepted.peer_cred().unwrap().uid(),
                    tentaflake_admission::effective_uid()
                );
                drop((accepted, client, unix));
                std::fs::remove_file(path).unwrap();
            });
    }
}
