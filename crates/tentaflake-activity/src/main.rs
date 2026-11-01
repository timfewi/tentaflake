fn main() {
    let result = (|| {
        let mut args = std::env::args().skip(1);
        let path = args.next().ok_or("activity config required")?;
        if args.next().is_some() {
            return Err("activity arguments");
        }
        let file = std::fs::File::open(path).map_err(|_| "activity config unavailable")?;
        let config = serde_json::from_reader(&file).map_err(|_| "activity config invalid")?;
        tentaflake_activity::require_root_config(&file)?;
        tentaflake_activity::serve_relay(config)
    })();
    if let Err(error) = result {
        eprintln!("tentaflake-activity: {error}");
        std::process::exit(1);
    }
}
