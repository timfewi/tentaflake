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
