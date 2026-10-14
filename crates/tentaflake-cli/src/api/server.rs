//! Unprivileged, bounded HTTP framing for the local API owner.
use super::config::Config;
use super::{MAX_BODY, MAX_CONNECTIONS, Reply, control, error, request_id};
use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::Request;
use hyper::body::Incoming;
use hyper::http::StatusCode;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

pub(super) fn serve(configuration: &Path) -> Result<(), String> {
    if unsafe { libc::geteuid() } != control::frontend_uid()? {
        return Err("host API HTTP transport requires its dedicated service user".into());
    }
    let config = Config::load(configuration)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "cannot initialize host API HTTP runtime")?
        .block_on(async {
            let origin: hyper::Uri = config
                .origin
                .parse()
                .map_err(|_| "invalid private API origin")?;
            let request = Request::builder()
                .uri("/api/meta")
                .header(
                    "host",
                    origin
                        .authority()
                        .ok_or("invalid private API origin")?
                        .as_str(),
                )
                .body(())
                .map_err(|_| "invalid private API origin")?;
            let ready = control::call(request.into_parts().0, Bytes::new())
                .await
                .map_err(|_| "host API owner is unavailable")?;
            if ready.status() != StatusCode::OK {
                return Err("host API owner refused startup".into());
            }
            let listener = TcpListener::bind(config.listen)
                .await
                .map_err(|_| "cannot bind host API loopback listener")?;
            let capacity = Arc::new(Semaphore::new(MAX_CONNECTIONS));
            loop {
                let (stream, _) = listener
                    .accept()
                    .await
                    .map_err(|_| "host API listener failed")?;
                let Ok(permit) = capacity.clone().try_acquire_owned() else {
                    continue;
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    connection(stream, control::call).await;
                });
            }
        })
}

async fn connection<F, Fut>(stream: TcpStream, forward: F)
where
    F: Fn(hyper::http::request::Parts, Bytes) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<Reply, ()>> + Send,
{
    let service = service_fn(move |request| receive(request, forward.clone()));
    let mut builder = http1::Builder::new();
    builder
        .keep_alive(false)
        .max_headers(64)
        .max_buf_size(32 * 1024)
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(5));
    let _ = tokio::time::timeout(
        Duration::from_secs(15),
        builder.serve_connection(TokioIo::new(stream), service),
    )
    .await;
}

async fn receive<F, Fut>(request: Request<Incoming>, forward: F) -> Result<Reply, Infallible>
where
    F: Fn(hyper::http::request::Parts, Bytes) -> Fut,
    Fut: Future<Output = Result<Reply, ()>>,
{
    let id = request_id();
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(body, MAX_BODY).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(_)) => return Ok(error(StatusCode::PAYLOAD_TOO_LARGE, "too_large", &id)),
        Err(_) => return Ok(error(StatusCode::REQUEST_TIMEOUT, "invalid", &id)),
    };
    // No fallback or replay: loss of the owner response is an unavailable request.
    Ok(forward(parts, body)
        .await
        .unwrap_or_else(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &id)))
}

#[cfg(test)]
mod tests {
    use super::super::owner;
    use super::*;
    use serde_json::{Value, json};
    use std::io::{Read, Write};

    fn client(
        address: std::net::SocketAddr,
        request: Vec<u8>,
    ) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
        std::thread::spawn(move || {
            let mut stream =
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            stream.write_all(&request)?;
            let mut response = Vec::new();
            stream.read_to_end(&mut response)?;
            Ok(response)
        })
    }

    async fn exchange(api: Arc<owner::Api>, request: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = client(listener.local_addr().unwrap(), request);
        let (stream, _) = listener.accept().await.unwrap();
        connection(stream, move |parts, body| {
            owner::exchange(api.clone(), parts, body)
        })
        .await;
        String::from_utf8(client.join().unwrap().unwrap()).unwrap()
    }

    #[test]
    fn real_http_discovery_and_refusals_are_bounded_and_never_cacheable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let api = owner::fixture();
            for (target,host,status) in [("/api/meta","host.example.invalid","200"),
                ("/api/meta","other.example.invalid","403"),
                ("/api/meta?ignored=1","host.example.invalid","400"),
                ("/api/v1/session/oidc","host.example.invalid","501"),
                ("/api/v1/session/oidc/authorize","host.example.invalid","501"),
                ("/api/v1/session/oidc/callback?code=synthetic","host.example.invalid","501")] {
                let reply = exchange(api.clone(),format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()).await;
                assert!(reply.starts_with(&format!("HTTP/1.1 {status}")),"{reply}");
                assert!(reply.contains("cache-control: no-store\r\n"),"{reply}");
                let body: Value = serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap();
                if status == "200" {
                    assert_eq!(body,json!({"service":"tentaflake","apiVersions":[1],"host":"host-fixture",
                        "auth":{"methods":["operator-token"]},"backend":"implemented"}));
                } else {
                    assert!(body.get("requestId").and_then(Value::as_str).is_some_and(super::super::config::id));
                    assert!(!reply.contains("uninstalled"));
                }
            }
            let mut request = format!("POST /api/v1/session HTTP/1.1\r\nHost: host.example.invalid\r\nContent-Length: {}\r\n\r\n",MAX_BODY+1).into_bytes();
            request.extend(vec![b'x'; MAX_BODY+1]);
            let reply = exchange(api,request).await;
            assert!(reply.starts_with("HTTP/1.1 413"),"{reply}");
            assert!(reply.contains("cache-control: no-store\r\n"));
        });
    }

    #[test]
    fn failed_owner_returns_unavailable_without_local_authentication() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = client(
                listener.local_addr().unwrap(),
                b"GET /api/meta HTTP/1.1\r\nHost: host.example.invalid\r\n\r\n".to_vec(),
            );
            let (stream, _) = listener.accept().await.unwrap();
            connection(stream, |_, _| async { Err(()) }).await;
            let reply = String::from_utf8(client.join().unwrap().unwrap()).unwrap();
            assert!(reply.starts_with("HTTP/1.1 503"), "{reply}");
            assert!(reply.contains("cache-control: no-store\r\n"));
            assert!(!reply.contains("set-cookie"));
        });
    }
}
