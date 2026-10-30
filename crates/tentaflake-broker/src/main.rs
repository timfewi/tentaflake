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
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("tentaflake-broker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let path = match parse_args()? {
        Command::Serve(path) => path,
        Command::InitializeStrictBudget(path) => return state::Limits::initialize_strict(&path),
        Command::ReconcileStrictBudget(path) => {
            let (unsettled, pending) = state::Limits::reconcile_strict(&path)?;
            println!(
                "resolved {unsettled} held reservations and {pending} usage holds without refunds"
            );
            return Ok(());
        }
    };
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
    let deadline = Instant::now() + settings.timeout;
    Some(std::thread::spawn(move || {
        let _guard = guard;
        handle_connection(
            &mut stream,
            broker,
            settings.header_limit,
            settings.body_limit,
            settings.timeout,
            deadline,
        );
    }))
}

fn handle_connection(
    stream: &mut TcpStream,
    broker: SharedBroker,
    header_limit: usize,
    body_limit: usize,
    timeout: Duration,
    deadline: Instant,
) {
    let _ = stream.set_write_timeout(Some(timeout));
    let response = match read_request(stream, header_limit, body_limit, deadline) {
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

enum Command {
    Serve(PathBuf),
    InitializeStrictBudget(PathBuf),
    ReconcileStrictBudget(PathBuf),
}

fn parse_args() -> Result<Command, String> {
    let mut args = env::args_os();
    let _program = args.next();
    let operation = args.next().ok_or("missing broker operation")?;
    let path = PathBuf::from(args.next().ok_or("missing absolute path")?);
    if args.next().is_some()
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("broker operation requires one absolute path without traversal".into());
    }
    match operation.to_str() {
        Some("--config") => Ok(Command::Serve(path)),
        Some("--initialize-strict-budget") => Ok(Command::InitializeStrictBudget(path)),
        Some("--reconcile-strict-budget") => Ok(Command::ReconcileStrictBudget(path)),
        _ => Err("usage: tentaflake-broker --config /absolute/path/config.json | --initialize-strict-budget /absolute/path/budget.json | --reconcile-strict-budget /absolute/path/budget.json".into()),
    }
}
