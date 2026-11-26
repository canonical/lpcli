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
        assert_eq!(
            run_lpcli(command),
            expected,
            "`lpcli {command}` must print SKILL.md byte for byte"
        );
    }
}

#[test]
fn help_and_version_are_available_at_startup() {
    let help = String::from_utf8(run_lpcli("--help")).expect("UTF-8 help text");
    assert!(help.contains("Usage: lpcli"), "{help}");
    assert!(help.contains("skills"), "{help}");
    assert_eq!(
        run_lpcli("--version"),
        concat!("lpcli ", env!("CARGO_PKG_VERSION"), "\n").as_bytes(),
    );
}

fn run_lpcli(argument: &str) -> Vec<u8> {
    let home = tempfile::tempdir().expect("create pristine HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lpcli"))
        .arg(argument)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("http_proxy", "http://127.0.0.1:9")
        .env("https_proxy", "http://127.0.0.1:9")
        .output()
        .expect("lpcli subprocess failed to start");
    assert!(
        output.status.success(),
        "`lpcli {argument}` exited with {:?}",
        output.status
    );
    assert!(
        output.stderr.is_empty(),
        "`lpcli {argument}` wrote to stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
