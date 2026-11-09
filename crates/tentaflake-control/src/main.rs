use std::io::Read;
use std::path::Path;
use tentaflake_control::{Config, Engine, storage::MAX_DOCUMENT};

fn run(args: &[String]) -> Result<i32, String> {
    let [mode, rest @ ..] = args else {
        return Err("usage: tentaflake-control serve <deployment.json> | template-fingerprint <immutable-template> | system | source | validate | propose | approve <proposal-id> | build | status <build-id>".into());
    };
    if mode == "serve" {
        let [path] = rest else {
            return Err("serve requires deployment configuration".into());
        };
        return tentaflake_control::transport::serve(Engine::open(Config::load(Path::new(path))?)?)
            .map(|_| 0);
    }
    if mode == "template-fingerprint" {
        let [path] = rest else {
            return Err("template-fingerprint requires one immutable template".into());
        };
        println!(
            "{}",
            tentaflake_control::template::fingerprint_for_review(Path::new(path))?
        );
        return Ok(0);
    }
    // Client deployment is fixed. There is deliberately no socket, executable, path or flags option.
    let config = Config::load(Path::new("/etc/tentaflake-control.json"))?;
    let (method, route) = match (mode.as_str(), rest) {
        ("system", []) => ("GET", "/v1/system".to_owned()),
        ("source", []) => ("GET", "/v1/system/source".to_owned()),
        ("validate", []) => ("POST", "/v1/system/validate".to_owned()),
        ("propose", []) => ("POST", "/v1/system/proposals".to_owned()),
        ("approve", [id]) => ("POST", format!("/v1/system/proposals/{id}/approve")),
        ("build", []) => ("POST", "/v1/system/builds".to_owned()),
        ("status", [id]) => ("GET", format!("/v1/system/builds/{id}")),
        _ => return Err("unsupported control operation".into()),
    };
    let mut body = Vec::new();
    if method == "POST" {
        std::io::stdin()
            .take((MAX_DOCUMENT + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|_| "request input unavailable")?;
        if body.len() > MAX_DOCUMENT {
            return Err("request input exceeds limit".into());
        }
    }
    let (status, response) = tentaflake_control::transport::client(&config, method, &route, &body)?;
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|_| "response unavailable")?
    );
    Ok(
        if status == 200 && (mode != "validate" || response["valid"] == true) {
            0
        } else {
            2
        },
    )
}
fn main() {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => std::process::exit(code),
        Err(_) => {
            eprintln!("tentaflake-control: operation refused or unavailable");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn fingerprint_is_dispatched_without_loading_deployment_configuration() {
        assert_eq!(
            run(&["template-fingerprint".into(), "/".into()]).unwrap_err(),
            "template must be immutable store source"
        );
        for arguments in [
            vec!["template-fingerprint".into()],
            vec!["template-fingerprint".into(), "/".into(), "extra".into()],
        ] {
            assert_eq!(
                run(&arguments).unwrap_err(),
                "template-fingerprint requires one immutable template"
            );
        }
    }
}
