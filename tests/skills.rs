//! Integration tests for the `skills` command: the agent skill documentation
//! (`SKILL.md`) is embedded in the binary at compile time and printed to
//! stdout byte for byte, and both root help forms surface it prominently.
//!
//! These tests only exercise the compiled binary and the repository files;
//! they perform no network access, no authentication, and no Launchpad API
//! calls.  Scratch directories live under `TMPDIR` (which the test environment
//! points at an approved scratch location), never at a fixed generic path.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The exact help text that both root help forms must show for `skills`.
const SKILLS_ABOUT: &str = "Agent Skills (coding agents should read this first)";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path to the repository-root `SKILL.md` from which the binary embeds the
/// skill documentation.
fn skill_md_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("SKILL.md")
}

/// Run the compiled `lpcli` binary with the given arguments and capture its
/// output.
fn run_lpcli(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lpcli"))
        .args(args)
        .output()
        .expect("lpcli subprocess failed to start")
}

/// Run the compiled `lpcli` binary with the given arguments inside a pristine
/// scratch `HOME` (no stored credentials, no configuration), proving the
/// invocation requires no authentication.
fn run_lpcli_without_auth(args: &[&str], label: &str) -> std::process::Output {
    let scratch_home = std::env::temp_dir()
        .join("lpcli-skills-tests")
        .join(format!("{}-{label}", std::process::id()));
    std::fs::create_dir_all(&scratch_home).expect("create scratch HOME directory");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lpcli"))
        .args(args)
        .env("HOME", &scratch_home)
        .env("XDG_CONFIG_HOME", scratch_home.join("config"))
        .output()
        .expect("lpcli subprocess failed to start");
    let _ = std::fs::remove_dir_all(&scratch_home);
    output
}

/// Assert a captured subprocess run succeeded and produced no stderr.
fn assert_clean_run(output: &std::process::Output, invocation: &str) {
    assert!(
        output.status.success(),
        "{invocation} exited with {:?}",
        output.status
    );
    assert!(
        output.stderr.is_empty(),
        "{invocation} wrote to stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Assert the given root help text lists `skills` prominently, with its
/// visible alias and the agent-facing description.
fn assert_root_help_lists_skills(stdout: &str, invocation: &str) {
    let commands_start = stdout
        .lines()
        .position(|line| line.trim() == "Commands:")
        .unwrap_or_else(|| panic!("{invocation} must have a Commands section: {stdout:?}"));
    let command_lines: Vec<&str> = stdout
        .lines()
        .skip(commands_start + 1)
        .take_while(|line| !line.trim().is_empty())
        .collect();

    let skills_index = command_lines
        .iter()
        .position(|line| line.split_whitespace().next() == Some("skills"))
        .unwrap_or_else(|| panic!("{invocation} must list the `skills` command: {stdout:?}"));
    assert!(
        skills_index <= 2,
        "`skills` must be listed prominently at the top of {invocation}, found at \
         position {skills_index}: {command_lines:?}"
    );

    let skills_line = command_lines[skills_index];
    assert!(
        skills_line.contains("[alias: skill]"),
        "{invocation} must show the visible alias `skill` alongside `skills`: \
         {skills_line:?}"
    );
    assert!(
        skills_line.contains(SKILLS_ABOUT),
        "{invocation} must describe skills for coding agents: {skills_line:?}"
    );
}

// ---------------------------------------------------------------------------
// Exact output
// ---------------------------------------------------------------------------

#[test]
fn skills_prints_the_embedded_skill_md_byte_for_byte() {
    let output = run_lpcli(&["skills"]);
    assert_clean_run(&output, "`lpcli skills`");

    // The subprocess stdout must be exactly the SKILL.md file bytes, with no
    // added newline, banner, or terminal formatting.
    let expected = std::fs::read(skill_md_path()).expect("read SKILL.md");
    assert!(
        !expected.is_empty(),
        "SKILL.md must not be empty; an empty document would make this test vacuous"
    );
    assert_eq!(output.stdout, expected);
}

#[test]
fn skill_alias_prints_the_same_documentation() {
    let canonical = run_lpcli(&["skills"]);
    assert_clean_run(&canonical, "`lpcli skills`");

    let alias = run_lpcli(&["skill"]);
    assert_clean_run(&alias, "`lpcli skill`");

    assert_eq!(
        alias.stdout, canonical.stdout,
        "`lpcli skill` must be a visible alias printing identical output"
    );
}

#[test]
fn skills_needs_no_credentials_or_home_config() {
    let output = run_lpcli_without_auth(&["skills"], "skills-no-home");
    let expected = std::fs::read(skill_md_path()).expect("read SKILL.md");
    assert_eq!(output.stdout, expected);
    assert_clean_run(&output, "`lpcli skills` with a pristine HOME");
}

// ---------------------------------------------------------------------------
// Help discoverability
// ---------------------------------------------------------------------------

#[test]
fn long_help_flag_lists_agent_skills_prominently() {
    let output = run_lpcli_without_auth(&["--help"], "long-help-flag");
    assert_clean_run(&output, "`lpcli --help`");
    assert_root_help_lists_skills(&String::from_utf8_lossy(&output.stdout), "`lpcli --help`");
}

#[test]
fn help_subcommand_lists_agent_skills_prominently() {
    let output = run_lpcli_without_auth(&["help"], "help-subcommand");
    assert_clean_run(&output, "`lpcli help`");
    assert_root_help_lists_skills(&String::from_utf8_lossy(&output.stdout), "`lpcli help`");
}

#[test]
fn skills_help_shows_the_agent_skills_description() {
    let canonical = run_lpcli(&["skills", "--help"]);
    assert_clean_run(&canonical, "`lpcli skills --help`");
    let stdout = String::from_utf8_lossy(&canonical.stdout);
    assert!(
        stdout.contains(SKILLS_ABOUT),
        "`lpcli skills --help` must show the agent skills description, got: {stdout:?}"
    );

    let alias = run_lpcli(&["skill", "--help"]);
    assert_clean_run(&alias, "`lpcli skill --help`");
    let alias_stdout = String::from_utf8_lossy(&alias.stdout);
    assert!(
        alias_stdout.contains(SKILLS_ABOUT),
        "`lpcli skill --help` must show the agent skills description, got: {alias_stdout:?}"
    );
}
