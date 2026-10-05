use std::process::Command;

#[test]
fn help_works_without_a_valid_host_configuration() {
    let missing = std::env::temp_dir().join(format!(
        "tentaflake-missing-help-config-{}",
        std::process::id()
    ));
    assert!(!missing.exists());

    for config in [missing, "/dev/null".into()] {
        for flag in ["help", "--help", "-h"] {
            let output = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
                .arg(flag)
                .env("TENTAFLAKE_CONFIG", &config)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{flag}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let help = String::from_utf8(output.stdout).unwrap();
            assert!(help.contains("USAGE"));
            assert!(help.contains("tentaflake doctor"));
        }
        let status = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
            .arg("status")
            .env("TENTAFLAKE_CONFIG", &config)
            .output()
            .unwrap();
        assert_eq!(status.status.code(), Some(2));
    }
}

#[test]
fn runtime_discovery_is_configuration_free_and_reports_scaffolds() {
    let output = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
        .args(["runtimes", "--json"])
        .env("TENTAFLAKE_CONFIG", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success());
    let catalog: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog["schemaVersion"], 1);
    assert_eq!(
        catalog["presets"]["openclaw"]["identity"]["status"],
        "scaffold"
    );
    assert_eq!(
        catalog["presets"]["openclaw"]["artifact"]["reference"],
        serde_json::Value::Null
    );
    assert_eq!(
        catalog["presets"]["hermes"]["evidence"]["vendorAcceptance"],
        "pending"
    );
    assert_eq!(
        catalog["presets"]["generic"]["identity"]["status"],
        "definition"
    );
    let invalid = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
        .args(["runtimes", "unexpected"])
        .env("TENTAFLAKE_CONFIG", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("runtimes accepts only"));
}

#[test]
fn templates_are_configuration_free_and_preserve_vendor_arguments_after_separator() {
    let image = format!("example.invalid/agent@sha256:{}", "a".repeat(64));
    let output = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
        .args([
            "agent", "template", "generic", "coding", "--image", &image, "--", "fixture", "--json",
            "--hide",
        ])
        .env("TENTAFLAKE_CONFIG", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success());
    let definition: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        definition["agents"][0]["definition"]["command"],
        serde_json::json!(["fixture", "--json", "--hide"])
    );
    assert_eq!(
        definition["agents"][0]["definition"]["lifecycle"],
        "stopped"
    );
}
