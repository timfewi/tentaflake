use std::process::Command;

#[test]
fn api_client_validates_before_host_configuration_credentials_or_network() {
    for (args, expected) in [
        (
            vec!["api", "--origin", "http://api.example.invalid", "meta"],
            "HTTPS",
        ),
        (
            vec!["api", "--origin", "https://api.example.invalid", "session"],
            "--bearer-file",
        ),
        (
            vec![
                "api",
                "--origin",
                "https://api.example.invalid",
                "--token",
                "synthetic-never-accepted-argv-secret",
                "session",
            ],
            "usage:",
        ),
        (
            vec![
                "api",
                "--origin",
                "https://api.example.invalid",
                "--bearer-file",
                "/dev/null",
                "session",
            ],
            "protected current-user",
        ),
        (
            vec![
                "api",
                "--origin",
                "https://api.example.invalid",
                "meta",
                "--hide",
            ],
            "--hide is not supported",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tentaflake"))
            .args(args)
            .env("TENTAFLAKE_CONFIG", "/dev/null")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected));
        assert!(!error.contains("synthetic-never-accepted-argv-secret"));
    }
}
