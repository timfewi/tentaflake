mod broker;
mod config;
mod http;
mod policy;
mod state;
mod streaming;

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
    let settings = ConnectionSettings {
        timeout: Duration::from_secs(config.timeout_seconds),
        max_concurrency: config.max_concurrency,
        header_limit: config.max_header_bytes,
        body_limit: config.max_request_bytes,
    };
    let broker = Arc::new(Broker::new(config)?);
    let active = Arc::new(AtomicUsize::new(0));

    for connection in listener.incoming() {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("tentaflake-broker: accept failed: {error}");
                continue;
            }
        };
        dispatch_connection(stream, Arc::clone(&broker), Arc::clone(&active), settings);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ConnectionSettings {
    timeout: Duration,
    max_concurrency: usize,
    header_limit: usize,
    body_limit: usize,
}

fn dispatch_connection(
    mut stream: TcpStream,
    broker: SharedBroker,
    active: Arc<AtomicUsize>,
    settings: ConnectionSettings,
) -> Option<std::thread::JoinHandle<()>> {
    let previous = active.fetch_add(1, Ordering::AcqRel);
    if previous >= settings.max_concurrency {
        active.fetch_sub(1, Ordering::AcqRel);
        let _ = stream.set_write_timeout(Some(settings.timeout));
        let _ = write_response(
            &mut stream,
            Response::json(503, json!({ "error": "broker concurrency limit exceeded" })),
        );
        return None;
    }
    let guard = ActiveGuard(active);
    Some(std::thread::spawn(move || {
        let _guard = guard;
        handle_connection(
            &mut stream,
            broker,
            settings.header_limit,
            settings.body_limit,
            settings.timeout,
        );
    }))
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
