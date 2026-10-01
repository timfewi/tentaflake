use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn executable(path: &Path, shell: &str, script: &str) {
    fs::write(path, format!("#!{shell}\n{script}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn inspect_output_never_bypasses_json_or_redaction() {
    let root = std::env::temp_dir().join(format!(
        "tentaflake-inspect-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let _cleanup = Fixture(root.clone());
    let shell = Command::new("sh")
        .args(["-c", "command -v sh"])
        .output()
        .unwrap();
    assert!(shell.status.success());
    let shell = String::from_utf8(shell.stdout).unwrap();
    let shell = shell.trim();
    executable(
        &root.join("sudo"),
        shell,
        r#"
printf '%s\n' "$*" >> "$INSPECT_CALLS"
printf '%s' 'fixture-stderr-secret' >&2
case "$3" in
    inspect)
        printf '%s' '[{"Config":{"Env":["TOKEN=fixture-env-secret"]}}]'
        ;;
    network)
        printf '%s' '[{"Name":"tf-inspect","Internal":true,"Driver":"bridge","fixture":"fixture-network-secret"}]'
        ;;
    *) exit 2 ;;
esac
"#,
    );
    executable(
        &root.join("df"),
        shell,
        "printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 100 1 99 1%% /\n'",
    );
    fs::write(root.join("agents.tsv"), "").unwrap();
    let mut fields = vec!["agent", "hermes-inspect-fixture", "balanced"];
    fields.extend(std::iter::repeat_n("true", 18));
    fields.extend(["false", "-", "true", "-", "tf-inspect"]);
    fs::write(
        root.join("security.tsv"),
        format!(
            "host\tbalanced\tfalse\ttrue\tfalse\ttrue\ttrue\tfalse\t36\n{}\n",
            fields.join("\t")
        ),
    )
    .unwrap();

    for backend in ["docker", "podman"] {
        fs::write(root.join("cli.conf"), format!(
            "backend={backend}\nflake_dir=/fixture\nhost_name=fixture\nsecurity_profile=balanced\nagents_file={}\nsecurity_file={}\n",
            root.join("agents.tsv").display(), root.join("security.tsv").display()
        )).unwrap();
        for flags in [vec!["--json"], vec!["--hide"], vec!["--json", "--hide"]] {
            let calls = root.join("calls");
            fs::write(&calls, "").unwrap();
            let result = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
                .args(["doctor", "--security"])
                .args(&flags)
                .env("TENTAFLAKE_CONFIG", root.join("cli.conf"))
                .env("INSPECT_CALLS", &calls)
                .env("PATH", &root)
                .output()
                .unwrap();
            // Security findings may return 1; parse/execution failures return 2.
            assert!(matches!(result.status.code(), Some(0 | 1)), "{result:?}");
            let stdout = String::from_utf8(result.stdout).unwrap();
            let stderr = String::from_utf8(result.stderr).unwrap();
            for secret in [
                "fixture-env-secret",
                "fixture-stderr-secret",
                "fixture-network-secret",
            ] {
                assert!(!stdout.contains(secret), "{stdout}");
                assert!(!stderr.contains(secret), "{stderr}");
            }
            if flags.contains(&"--json") {
                let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                assert!(value["findings"].is_array());
            }
            if flags.contains(&"--hide") {
                assert!(!stdout.contains("hermes-inspect-fixture"), "{stdout}");
                assert!(stdout.contains("redacted"), "{stdout}");
            }
            let calls = fs::read_to_string(&calls).unwrap();
            assert_eq!(
                calls,
                format!(
                    "-n {backend} inspect hermes-inspect-fixture\n-n {backend} network inspect tf-inspect\n"
                )
            );
        }
    }
}
