use std::path::Path;
use tentaflake_integrations::front::{Server, load_config};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.as_slice() {
        [_, command, path] if command == "serve" || command == "serve-advisory" => {
            load_config(Path::new(path))
                .and_then(|config| Server::new(config, command == "serve-advisory"))
                .and_then(Server::serve)
        }
        _ => {
            Err("usage: tentaflake-integrations serve|serve-advisory /absolute/runtime-config.json")
        }
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
