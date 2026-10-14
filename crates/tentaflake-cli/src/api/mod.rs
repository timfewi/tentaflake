//! Private HTTP transport for the host-owned management API.
//! Authentication never grants lifecycle dispatch; admission owns durable state.
mod capabilities;
mod catalog;
mod client;
mod config;
mod control;
mod creation;
mod credential;
mod operations;
mod owner;
mod planning;
mod server;
mod session;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{
    Response,
    http::{HeaderValue, StatusCode},
};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};
use std::path::Path;

pub(super) type Reply = Response<Full<Bytes>>;
pub(super) const MAX_CONNECTIONS: usize = 32;

pub(super) const MAX_BODY: usize = 32 * 1024;

pub(crate) fn run(args: &[String], mode: crate::OutputMode) -> Result<u8, String> {
    match args.first().map(String::as_str) {
        Some("serve") | Some("owner") => {}
        _ => return client::run(args, mode),
    }
    if mode.hide || mode.json {
        return Err("host API services do not accept output flags".into());
    }
    match args {
        [command, configuration] if command == "serve" => server::serve(Path::new(configuration))?,
        [command, configuration, plan] if command == "owner" => owner::serve(Path::new(configuration), Path::new(plan))?,
        _ => return Err("usage: tentaflake api serve <generated-store-config> | owner <generated-store-config> <generated-store-plan>".into()),
    }
    Ok(0)
}

pub(super) fn response(status: StatusCode, body: Bytes) -> Reply {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub(super) fn json_reply(status: StatusCode, body: Value) -> Reply {
    let mut response = response(status, Bytes::from(body.to_string()));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

pub(super) fn error(status: StatusCode, code: &str, request_id: &str) -> Reply {
    json_reply(
        status,
        json!({"error":{"code":code,"message":"The host API could not accept this request."},"requestId":request_id}),
    )
}

pub(super) fn request_id() -> String {
    use std::fmt::Write;
    let mut bytes = [0; 16];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return "request-unavailable".into();
    }
    let mut id = String::from("request-");
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("string formatting");
    }
    id
}
