use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tentaflake-diagnostics-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let shell = Command::new("sh")
            .args(["-c", "command -v sh"])
            .output()
            .unwrap();
        assert!(shell.status.success());
        let shell = String::from_utf8(shell.stdout).unwrap();
        for (name, script) in [
            (
                "systemctl",
                r#"printf 'fixture-private-stderr\n' >&2
case "$1" in
  --failed) printf '%s' "$FIXTURE_FAILED_OUTPUT"; exit "$FIXTURE_FAILED_EXIT" ;;
  is-active)
    if [ "$2" = stopped-fixture.service ]; then printf 'inactive\n'; exit 3; fi
    printf '%s\n' "$FIXTURE_STATE"; exit "$FIXTURE_STATE_EXIT" ;;
  *) exit 2 ;;
esac"#,
            ),
            (
                "df",
                r#"printf 'fixture-private-stderr\n' >&2
printf '%s\n' "$FIXTURE_DISK"
exit "$FIXTURE_DISK_EXIT""#,
            ),
        ] {
            let path = root.join(name);
            fs::write(&path, format!("#!{}\n{script}\n", shell.trim())).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            root.join("agents.tsv"),
            "hermes\tcoding-private-fixture\tcoding-fixture\tcoding-fixture.service\t/tmp/coding-fixture\n\
             hermes\tstopped-private-fixture\tstopped-fixture\tstopped-fixture.service\t/tmp/stopped-fixture\n",
        )
        .unwrap();
        fs::write(
            root.join("cli.conf"),
            format!(
                "backend=docker\nflake_dir=/tmp\nhost_name=fixture-private-host\nagents_file={}\n",
                root.join("agents.tsv").display()
            ),
        )
        .unwrap();
        Self(root)
    }

    fn run(&self, command: &str, json: bool, hide: bool, changes: &[(&str, &str)]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tentaflake"));
        child
            .env_clear()
            .env("PATH", &self.0)
            .env("TENTAFLAKE_CONFIG", self.0.join("cli.conf"))
            .env("FIXTURE_FAILED_OUTPUT", "")
            .env("FIXTURE_FAILED_EXIT", "0")
            .env("FIXTURE_STATE", "active")
            .env("FIXTURE_STATE_EXIT", "0")
            .env("FIXTURE_DISK", "Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture-device 100 1 99 1% /")
            .env("FIXTURE_DISK_EXIT", "0")
            .envs(changes.iter().copied())
            .arg(command);
        if json {
            child.arg("--json");
        }
        if hide {
            child.arg("--hide");
        }
        child.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hidden_diagnostics_redact_names_in_both_formats() {
    let fixture = Fixture::new();
    for command in ["doctor", "health"] {
        for json in [false, true] {
            let output = fixture.run(
                command,
                json,
                true,
                &[("FIXTURE_STATE", "failed"), ("FIXTURE_STATE_EXIT", "3")],
            );
            assert_eq!(output.status.code(), Some(1));
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                !stdout.contains("private"),
                "{command}, json={json}: {stdout}"
            );
            assert!(output.stderr.is_empty());
            if json {
                let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                assert_eq!(parsed["host"], "redacted");
                assert_eq!(parsed["failed_agents"], serde_json::json!(["agent-1"]));
            } else if command == "doctor" {
                assert!(stdout.contains("agent-1"));
            }
        }
    }
}

#[test]
fn unavailable_host_evidence_never_reports_success() {
    let fixture = Fixture::new();
    for command in ["doctor", "health"] {
        for json in [false, true] {
            for changes in [
                vec![("FIXTURE_FAILED_EXIT", "1")],
                vec![("FIXTURE_DISK_EXIT", "1")],
                vec![("FIXTURE_DISK", "invalid disk output")],
            ] {
                let output = fixture.run(command, json, true, &changes);
                assert_eq!(
                    output.status.code(),
                    Some(2),
                    "{command}, json={json}, changes={changes:?}"
                );
                assert!(
                    !String::from_utf8(output.stderr)
                        .unwrap()
                        .contains("private")
                );
            }
        }
    }
}

#[test]
fn unknown_agent_state_is_a_problem_but_stopped_agents_are_valid() {
    let fixture = Fixture::new();
    for command in ["doctor", "health"] {
        for json in [false, true] {
            let healthy = fixture.run(command, json, false, &[]);
            assert!(healthy.status.success());
            let stdout = String::from_utf8(healthy.stdout).unwrap();
            assert!(stdout.contains("fixture-private-host"));
            if json {
                let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                assert_eq!(parsed["problems"], 0);
                assert_eq!(parsed["failed_agents"], serde_json::json!([]));
                assert_eq!(parsed["unknown_agents"], serde_json::json!([]));
            }
            for (state, code) in [
                ("", "1"),
                ("fixture-private-invalid-state", "1"),
                ("active", "1"),
                ("inactive", "4"),
            ] {
                let unknown = fixture.run(
                    command,
                    json,
                    true,
                    &[("FIXTURE_STATE", state), ("FIXTURE_STATE_EXIT", code)],
                );
                assert_eq!(
                    unknown.status.code(),
                    Some(1),
                    "{command}, json={json}, state={state:?}"
                );
                assert!(!String::from_utf8_lossy(&unknown.stdout).contains("private"));
                if json {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&unknown.stdout).unwrap();
                    assert_eq!(parsed["unknown_agents"], serde_json::json!(["agent-1"]));
                }
            }
        }
    }
}

#[test]
fn diagnoses_host_failures_and_disk_pressure() {
    let fixture = Fixture::new();
    for command in ["doctor", "health"] {
        for json in [false, true] {
            for changes in [
                vec![(
                    "FIXTURE_FAILED_OUTPUT",
                    "fixture.service loaded failed failed",
                )],
                vec![(
                    "FIXTURE_DISK",
                    "Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture-device 100 90 10 90% /",
                )],
            ] {
                let output = fixture.run(command, json, false, &changes);
                assert_eq!(output.status.code(), Some(1), "{command}, json={json}");
            }
        }
    }
}
