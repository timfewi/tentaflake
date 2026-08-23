mod broker;
mod config;
mod http;
mod policy;
mod state;

use broker::{Broker, SharedBroker};
use config::Config;
use http::{Response, read_request, write_response};
use serde_json::json;
use std::env;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("tentaflake-broker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let path = parse_args()?;
    let config = Config::load(&path)?;
    let listener = TcpListener::bind(config.listen)
        .map_err(|error| format!("cannot bind broker listener {}: {error}", config.listen))?;
    let timeout = Duration::from_secs(config.timeout_seconds);
    let max_concurrency = config.max_concurrency;
    let header_limit = config.max_header_bytes;
    let body_limit = config.max_request_bytes;
    let broker = Arc::new(Broker::new(config)?);
    let active = Arc::new(AtomicUsize::new(0));

    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("tentaflake-broker: accept failed: {error}");
                continue;
            }
        };
        let previous = active.fetch_add(1, Ordering::AcqRel);
        if previous >= max_concurrency {
            active.fetch_sub(1, Ordering::AcqRel);
            let _ = write_response(
                &mut stream,
                Response::json(503, json!({ "error": "broker concurrency limit exceeded" })),
            );
            continue;
        }
        let broker = Arc::clone(&broker);
        let active = Arc::clone(&active);
        std::thread::spawn(move || {
            let _guard = ActiveGuard(active);
            handle_connection(&mut stream, broker, header_limit, body_limit, timeout);
        });
    }
    Ok(())
}

fn handle_connection(
    stream: &mut TcpStream,
    broker: SharedBroker,
    header_limit: usize,
    body_limit: usize,
    timeout: Duration,
) {
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let response = match read_request(stream, header_limit, body_limit) {
        Ok(request) => broker.handle(request),
        Err(reason) => Response::json(400, json!({ "error": reason })),
    };
    let _ = write_response(stream, response);
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn parse_args() -> Result<PathBuf, String> {
    let mut args = env::args_os();
    let _program = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--config")) {
        return Err("usage: tentaflake-broker --config /absolute/path/config.json".into());
    }
    let path = PathBuf::from(args.next().ok_or("missing --config path")?);
    if args.next().is_some() || !path.is_absolute() {
        return Err("--config requires one absolute path".into());
    }
    Ok(path)
}
