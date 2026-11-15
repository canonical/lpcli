//! Integration test for the `skills` command: the agent skill documentation
//! (`SKILL.md`) embedded in the binary at compile time is printed to stdout
//! byte for byte, with no credentials, configuration, or network access.

#[test]
fn skills_and_its_alias_print_the_embedded_skill_md_byte_for_byte() {
    let expected: &[u8] = include_bytes!("../SKILL.md");
    assert!(
        !expected.is_empty(),
        "SKILL.md must not be empty; an empty document would make this test vacuous"
    );

    for command in ["skills", "skill"] {
        // Pristine HOME/XDG_CONFIG_HOME and a dead proxy prove the command
        // needs no stored credentials, configuration, or network access.
        let home = tempfile::tempdir().expect("create pristine HOME");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lpcli"))
            .args([command])
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("http_proxy", "http://127.0.0.1:9")
            .env("https_proxy", "http://127.0.0.1:9")
            .output()
            .expect("lpcli subprocess failed to start");
        assert!(
            output.status.success(),
            "`lpcli {command}` exited with {:?}",
            output.status
        );
        assert!(
            output.stderr.is_empty(),
            "`lpcli {command}` wrote to stderr: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout, expected,
            "`lpcli {command}` must print SKILL.md byte for byte"
        );
    }
}
