//! Private HTTP transport for the host-owned management API.
//! Authentication never grants lifecycle dispatch; admission owns durable state.
mod capabilities;
mod client;
mod config;
mod credential;
mod server;
mod session;

use std::path::Path;

pub(super) const MAX_BODY: usize = 32 * 1024;

pub(crate) fn run(args: &[String], mode: crate::OutputMode) -> Result<u8, String> {
    if args.first().map(String::as_str) != Some("serve") {
        return client::run(args, mode);
    }
    let [command, configuration, plan] = args else {
        return Err(
            "usage: tentaflake api serve <generated-store-config> <generated-store-plan>".into(),
        );
    };
    if command != "serve" || mode.hide || mode.json {
        return Err(
            "api serve requires two immutable configuration paths and no output flags".into(),
        );
    }
    if unsafe { libc::geteuid() } != 0 {
        return Err("host API service requires root".into());
    }
    server::serve(Path::new(configuration), Path::new(plan))?;
    Ok(0)
}
