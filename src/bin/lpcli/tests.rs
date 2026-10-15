//! Unit tests for the `lpcli` binary: CLI argument parsing and helper
//! functions used by the command handlers.

use super::*;

// -----------------------------------------------------------------------
// Helpers shared by multiple tests
// -----------------------------------------------------------------------

/// Build a minimal `BugTask` whose only meaningful field is `target_link`.
fn make_task(target_link: &str) -> bugs::BugTask {
    bugs::BugTask {
        self_link: Some(format!(
            "https://api.launchpad.net/devel{target_link}/+bugtask"
        )),
        bug_link: Some("https://api.launchpad.net/devel/bugs/1".to_string()),
        title: Some("Test bug".to_string()),
        status: Some("New".to_string()),
        importance: Some("Undecided".to_string()),
        assignee_link: None,
        date_created: None,
        bug_target_display_name: None,
        target_link: Some(format!("https://api.launchpad.net/devel{target_link}")),
        bug_watch_link: None,
    }
}

/// A representative set of tasks covering several targets, series, and
/// also series-less rows (distribution-level and upstream project tasks).
fn sample_tasks() -> Vec<bugs::BugTask> {
    vec![
        // rust-alacritty: noble and jammy only
        make_task("/ubuntu/noble/+source/rust-alacritty"),
        make_task("/ubuntu/jammy/+source/rust-alacritty"),
        // rust-eza: noble, jammy, and focal
        make_task("/ubuntu/noble/+source/rust-eza"),
        make_task("/ubuntu/jammy/+source/rust-eza"),
        make_task("/ubuntu/focal/+source/rust-eza"),
        // Series-less rows — included when SeriesFilter::All is used
        make_task("/ubuntu/+source/rust-alacritty"),
        make_task("/rust-alacritty"),
    ]
}

/// Extract the target_link strings from a slice of task references so that
/// test assertions stay readable.
fn target_links<'a>(tasks: &'a [&'a bugs::BugTask]) -> Vec<&'a str> {
    tasks
        .iter()
        .filter_map(|t| t.target_link.as_deref())
        .collect()
}

// -----------------------------------------------------------------------
// parse_comma_separated
// -----------------------------------------------------------------------

#[test]
fn parse_comma_separated_single() {
    assert_eq!(parse_comma_separated("noble"), vec!["noble"]);
}

#[test]
fn parse_comma_separated_two() {
    assert_eq!(
        parse_comma_separated("noble, jammy"),
        vec!["noble", "jammy"]
    );
}

#[test]
fn parse_comma_separated_trims_whitespace() {
    assert_eq!(
        parse_comma_separated("  noble  ,  jammy  "),
        vec!["noble", "jammy"]
    );
}

#[test]
fn parse_comma_separated_ignores_empty_segments() {
    assert_eq!(
        parse_comma_separated(",noble,,jammy,"),
        vec!["noble", "jammy"]
    );
}

// -----------------------------------------------------------------------
// TargetFilter::from_args
// -----------------------------------------------------------------------

#[test]
fn target_filter_from_all_targets_flag() {
    let f = TargetFilter::from_args(None, None, true);
    assert_eq!(f, TargetFilter::All);
}

#[test]
fn target_filter_from_many_targets() {
    let f = TargetFilter::from_args(None, Some("rust-alacritty, rust-eza".into()), false);
    assert_eq!(
        f,
        TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()])
    );
}

#[test]
fn target_filter_from_single_target() {
    let f = TargetFilter::from_args(Some("rust-alacritty".into()), None, false);
    assert_eq!(f, TargetFilter::One("rust-alacritty".into()));
}

// -----------------------------------------------------------------------
// SeriesFilter::from_args
// -----------------------------------------------------------------------

#[test]
fn series_filter_from_all_series_flag() {
    let f = SeriesFilter::from_args(None, None, true, false);
    assert_eq!(f, SeriesFilter::All);
}

#[test]
fn series_filter_from_many_series() {
    let f = SeriesFilter::from_args(None, Some("noble, jammy".into()), false, false);
    assert_eq!(f, SeriesFilter::Many(vec!["noble".into(), "jammy".into()]));
}

#[test]
fn series_filter_from_single_series() {
    let f = SeriesFilter::from_args(Some("noble".into()), None, false, false);
    assert_eq!(f, SeriesFilter::One("noble".into()));
}

#[test]
fn series_filter_from_no_series_flag() {
    let f = SeriesFilter::from_args(None, None, false, true);
    assert_eq!(f, SeriesFilter::NoSeries);
}

// -----------------------------------------------------------------------
// collect_matching_tasks — successful combinations
// -----------------------------------------------------------------------

#[test]
fn collect_one_target_one_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
    assert!(target_links(&result)[0].contains("noble/+source/rust-alacritty"));
}

#[test]
fn collect_one_target_one_series_case_insensitive() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("Rust-Alacritty".into()),
        &SeriesFilter::One("Noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn collect_one_target_many_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_many_targets_one_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_many_targets_many_series() {
    let tasks = sample_tasks();
    // rust-alacritty × {noble, jammy} + rust-eza × {noble, jammy} = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
}

#[test]
fn collect_all_targets_one_series() {
    let tasks = sample_tasks();
    // noble has rust-alacritty and rust-eza → 2 tasks
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::All,
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_one_target_all_series() {
    let tasks = sample_tasks();
    // rust-alacritty: noble, jammy (series-specific) + ubuntu/+source (series-less)
    // + /rust-alacritty (project task, also series-less) = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::All,
    )
    .unwrap();
    assert_eq!(result.len(), 4);
    let links = target_links(&result);
    assert!(links.iter().any(|l| l.contains("noble")));
    assert!(links.iter().any(|l| l.contains("jammy")));
    assert!(
        links
            .iter()
            .any(|l| l.contains("/ubuntu/+source/rust-alacritty"))
    );
    assert!(links.iter().any(|l| l.ends_with("/rust-alacritty")));
}

#[test]
fn collect_all_targets_many_series() {
    let tasks = sample_tasks();
    // noble + jammy: rust-alacritty × 2 + rust-eza × 2 = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::All,
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
}

#[test]
fn collect_all_targets_all_series() {
    let tasks = sample_tasks();
    // Series-specific: noble/alacritty, jammy/alacritty, noble/eza, jammy/eza, focal/eza = 5
    // Series-less: ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    // Total = 7
    let result = collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::All).unwrap();
    assert_eq!(result.len(), 7);
}

#[test]
fn collect_many_targets_all_series() {
    let tasks = sample_tasks();
    // rust-eza: noble, jammy, focal = 3 (no series-less row)
    // rust-alacritty: noble, jammy = 2 + ubuntu/+source (series-less) + /rust-alacritty = 4
    // Total = 7
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::All,
    )
    .unwrap();
    assert_eq!(result.len(), 7);
}

// -----------------------------------------------------------------------
// collect_matching_tasks — error cases
// -----------------------------------------------------------------------

#[test]
fn collect_invalid_single_target_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("nonexistent-pkg".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nonexistent-pkg"), "msg: {msg}");
    assert!(msg.contains("42"), "msg: {msg}");
    assert!(
        msg.contains("rust-alacritty") || msg.contains("rust-eza"),
        "msg: {msg}"
    );
}

#[test]
fn collect_invalid_one_of_many_targets_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "ghost-pkg".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    // The missing target must be named in the error.
    assert!(msg.contains("ghost-pkg"), "msg: {msg}");
    // The valid targets must appear in the 'Available targets' list.
    assert!(msg.contains("rust-alacritty"), "msg: {msg}");
}

#[test]
fn collect_all_of_many_targets_invalid_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::Many(vec!["ghost-a".into(), "ghost-b".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ghost-a") && msg.contains("ghost-b"),
        "msg: {msg}"
    );
}

#[test]
fn collect_invalid_single_series_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("groovy".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("groovy"), "msg: {msg}");
    assert!(msg.contains("noble") || msg.contains("jammy"), "msg: {msg}");
}

#[test]
fn collect_invalid_one_of_many_series_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::Many(vec!["noble".into(), "eoan".into()]),
    )
    .unwrap_err();
    let msg = err.to_string();
    // The missing series must be named in the error.
    assert!(msg.contains("eoan"), "msg: {msg}");
    // The valid series must appear in the 'Available series' list.
    assert!(msg.contains("noble"), "msg: {msg}");
}

#[test]
fn collect_valid_target_and_series_but_no_intersection_returns_error() {
    // rust-alacritty has no focal task; rust-eza has focal.
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("focal".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("No tasks found"), "msg: {msg}");
}

#[test]
fn collect_one_target_no_series() {
    let tasks = sample_tasks();
    // rust-alacritty series-less rows: /ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::NoSeries,
    )
    .unwrap();
    assert_eq!(result.len(), 2);
    for link in target_links(&result) {
        assert!(
            !link.contains("/noble/") && !link.contains("/jammy/") && !link.contains("/focal/"),
            "expected no series in link: {link}"
        );
    }
}

#[test]
fn collect_all_targets_no_series() {
    let tasks = sample_tasks();
    // Only series-less tasks in sample_tasks: /ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    let result =
        collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::NoSeries).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_series_less_tasks_included_by_all_series() {
    // SeriesFilter::All includes both series-specific and series-less tasks.
    let tasks = vec![
        make_task("/ubuntu/+source/rust-alacritty"),
        make_task("/rust-alacritty"),
        make_task("/ubuntu/noble/+source/rust-alacritty"),
    ];
    let result = collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::All).unwrap();
    assert_eq!(result.len(), 3);
    let links = target_links(&result);
    assert!(links.iter().any(|l| l.contains("noble")));
    assert!(
        links
            .iter()
            .any(|l| l.contains("/ubuntu/+source/rust-alacritty"))
    );
    assert!(links.iter().any(|l| l.ends_with("/rust-alacritty")));
}

#[test]
fn collect_empty_tasks_list_returns_error() {
    let tasks: Vec<bugs::BugTask> = vec![];
    let err = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    // With no tasks the target validation fires first.
    assert!(err.to_string().contains("rust-alacritty"));
}

// -----------------------------------------------------------------------
// truncate / build_table (pre-existing)
// -----------------------------------------------------------------------

#[test]
fn truncate_short_string_unchanged() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_long_string_adds_ellipsis() {
    let s = truncate("abcdefghij", 5);
    assert!(s.ends_with('…'));
    assert!(s.chars().count() <= 5);
}

#[test]
fn truncate_exact_length_unchanged() {
    assert_eq!(truncate("hello", 5), "hello");
}

#[test]
fn build_table_smoke() {
    let table = build_table(vec!["Col A", "Col B"]);
    let rendered = table.to_string();
    assert!(rendered.contains("Col A"));
    assert!(rendered.contains("Col B"));
}

// -----------------------------------------------------------------------
// parse_tags_from_vec
// -----------------------------------------------------------------------

#[test]
fn parse_tags_from_vec_space_separated() {
    let args = vec!["homestar".into(), "cheat".into(), "strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_single_comma_separated_string() {
    let args = vec!["homestar, cheat, strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_shell_split_trailing_commas() {
    // Simulates what the shell hands to the process when the user types:
    //   --add-many-tags homestar, cheat, strongbad
    let args = vec!["homestar,".into(), "cheat,".into(), "strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_filters_empty_segments() {
    let args = vec![",,,".into(), "  ".into(), "  java  ".into()];
    assert_eq!(parse_tags_from_vec(&args), vec!["java"]);
}

// -----------------------------------------------------------------------
// apply_tag_removals
// -----------------------------------------------------------------------

#[test]
fn apply_tag_removals_single_tag() {
    let current = vec!["apple".into(), "banana".into(), "pear".into()];
    let result = apply_tag_removals(current, Some("pear".into()), None, false);
    assert_eq!(result, vec!["apple", "banana"]);
}

#[test]
fn apply_tag_removals_many_tags_space_separated() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let result = apply_tag_removals(
        current,
        None,
        Some(vec!["lettuce".into(), "tomato".into()]),
        false,
    );
    assert_eq!(result, vec!["cucumber"]);
}

#[test]
fn apply_tag_removals_many_tags_comma_separated_string() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let result = apply_tag_removals(current, None, Some(vec!["lettuce, tomato".into()]), false);
    assert_eq!(result, vec!["cucumber"]);
}

#[test]
fn apply_tag_removals_all_tags() {
    let current = vec!["spam".into(), "eggs".into(), "sausage".into()];
    let result = apply_tag_removals(current, None, None, true);
    assert!(result.is_empty());
}

#[test]
fn apply_tag_removals_all_tags_ignores_other_flags() {
    // remove_all_tags takes precedence over the specific remove flags.
    let current = vec!["spam".into(), "eggs".into()];
    let result = apply_tag_removals(
        current,
        Some("spam".into()),
        Some(vec!["eggs".into()]),
        true,
    );
    assert!(result.is_empty());
}

#[test]
fn apply_tag_removals_no_flags_returns_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_removals(current.clone(), None, None, false);
    assert_eq!(result, current);
}

#[test]
fn apply_tag_removals_case_insensitive() {
    let current = vec!["Apple".into(), "Banana".into()];
    let result = apply_tag_removals(current, Some("APPLE".into()), None, false);
    assert_eq!(result, vec!["Banana"]);
}

#[test]
fn apply_tag_removals_nonexistent_tag_leaves_list_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_removals(current.clone(), Some("gamma".into()), None, false);
    assert_eq!(result, current);
}

// -----------------------------------------------------------------------
// apply_tag_additions
// -----------------------------------------------------------------------

#[test]
fn apply_tag_additions_single_tag() {
    let current = vec!["existing".into()];
    let result = apply_tag_additions(current, Some("java".into()), None);
    assert_eq!(result, vec!["existing", "java"]);
}

#[test]
fn apply_tag_additions_many_tags_space_separated() {
    let current: Vec<String> = vec![];
    let result = apply_tag_additions(
        current,
        None,
        Some(vec!["homestar".into(), "cheat".into(), "strongbad".into()]),
    );
    assert_eq!(result, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn apply_tag_additions_many_tags_comma_separated_string() {
    let current: Vec<String> = vec![];
    let result = apply_tag_additions(
        current,
        None,
        Some(vec!["homestar, cheat, strongbad".into()]),
    );
    assert_eq!(result, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn apply_tag_additions_no_flags_returns_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_additions(current.clone(), None, None);
    assert_eq!(result, current);
}

#[test]
fn apply_tag_additions_skips_duplicates() {
    let current = vec!["java".into(), "python".into()];
    let result = apply_tag_additions(current, Some("java".into()), None);
    // "java" must not appear twice.
    assert_eq!(result, vec!["java", "python"]);
}

#[test]
fn apply_tag_additions_case_insensitive_duplicate_check() {
    let current = vec!["Java".into()];
    let result = apply_tag_additions(current, Some("JAVA".into()), None);
    // Case-insensitive match: "JAVA" is already present as "Java".
    assert_eq!(result, vec!["Java"]);
}

// -----------------------------------------------------------------------
// remove + add combined
// -----------------------------------------------------------------------

#[test]
fn remove_all_then_add_many() {
    // Bug has: baseball, football, basketball
    // --remove-all-tags --add-many-tags "cricket, badminton"
    let current = vec!["baseball".into(), "football".into(), "basketball".into()];
    let after_remove = apply_tag_removals(current, None, None, true);
    let final_tags =
        apply_tag_additions(after_remove, None, Some(vec!["cricket, badminton".into()]));
    assert_eq!(final_tags, vec!["cricket", "badminton"]);
}

#[test]
fn remove_one_then_add_one() {
    let current = vec!["apple".into(), "banana".into(), "pear".into()];
    let after_remove = apply_tag_removals(current, Some("pear".into()), None, false);
    let final_tags = apply_tag_additions(after_remove, Some("mango".into()), None);
    assert_eq!(final_tags, vec!["apple", "banana", "mango"]);
}

#[test]
fn remove_many_then_add_single() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let after_remove = apply_tag_removals(
        current,
        None,
        Some(vec!["lettuce".into(), "tomato".into()]),
        false,
    );
    let final_tags = apply_tag_additions(after_remove, Some("spinach".into()), None);
    assert_eq!(final_tags, vec!["cucumber", "spinach"]);
}

// -----------------------------------------------------------------------
// CLI argument parsing — Tag subcommand
// -----------------------------------------------------------------------

/// Helper: parse a `BugCommand::Tag` from CLI tokens and return the variant.
fn parse_tag_cmd(args: &[&str]) -> BugCommand {
    let full: Vec<&str> = std::iter::once("lpcli")
        .chain(std::iter::once("bug"))
        .chain(std::iter::once("tag"))
        .chain(args.iter().copied())
        .collect();
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    match cli.command {
        Command::Bug(cmd) => cmd,
        _ => panic!("Expected Bug command"),
    }
}

#[test]
fn cli_tag_add_tag_single() {
    let cmd = parse_tag_cmd(&["--bug-id", "42", "--add-tag", "java"]);
    let BugCommand::Tag {
        bug_id,
        add_tag,
        add_many_tags,
        remove_tag,
        remove_many_tags,
        remove_all_tags,
    } = cmd
    else {
        panic!("Expected Tag variant");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(add_tag, Some("java".to_string()));
    assert!(add_many_tags.is_none());
    assert!(remove_tag.is_none());
    assert!(remove_many_tags.is_none());
    assert!(!remove_all_tags);
}

#[test]
fn cli_tag_add_many_tags_space_separated() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "42",
        "--add-many-tags",
        "homestar",
        "cheat",
        "strongbad",
    ]);
    let BugCommand::Tag { add_many_tags, .. } = cmd else {
        panic!()
    };
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn cli_tag_add_many_tags_comma_separated() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "42",
        "--add-many-tags",
        "homestar, cheat, strongbad",
    ]);
    let BugCommand::Tag { add_many_tags, .. } = cmd else {
        panic!()
    };
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn cli_tag_remove_tag_single() {
    let cmd = parse_tag_cmd(&["--bug-id", "99", "--remove-tag", "pear"]);
    let BugCommand::Tag {
        remove_tag,
        remove_all_tags,
        ..
    } = cmd
    else {
        panic!()
    };
    assert_eq!(remove_tag, Some("pear".to_string()));
    assert!(!remove_all_tags);
}

#[test]
fn cli_tag_remove_many_tags() {
    let cmd = parse_tag_cmd(&["--bug-id", "99", "--remove-many-tags", "lettuce", "tomato"]);
    let BugCommand::Tag {
        remove_many_tags, ..
    } = cmd
    else {
        panic!()
    };
    let tags = parse_tags_from_vec(&remove_many_tags.unwrap());
    assert_eq!(tags, vec!["lettuce", "tomato"]);
}

#[test]
fn cli_tag_remove_all_tags() {
    let cmd = parse_tag_cmd(&["--bug-id", "7", "--remove-all-tags"]);
    let BugCommand::Tag {
        remove_all_tags,
        remove_tag,
        remove_many_tags,
        ..
    } = cmd
    else {
        panic!()
    };
    assert!(remove_all_tags);
    assert!(remove_tag.is_none());
    assert!(remove_many_tags.is_none());
}

#[test]
fn cli_tag_remove_all_and_add_many() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "1",
        "--remove-all-tags",
        "--add-many-tags",
        "cricket",
        "badminton",
    ]);
    let BugCommand::Tag {
        remove_all_tags,
        add_many_tags,
        add_tag,
        ..
    } = cmd
    else {
        panic!()
    };
    assert!(remove_all_tags);
    assert!(add_tag.is_none());
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["cricket", "badminton"]);
}

#[test]
fn cli_tag_add_tag_and_remove_tag_together() {
    let cmd = parse_tag_cmd(&["--bug-id", "1", "--remove-tag", "old", "--add-tag", "new"]);
    let BugCommand::Tag {
        remove_tag,
        add_tag,
        ..
    } = cmd
    else {
        panic!()
    };
    assert_eq!(remove_tag, Some("old".to_string()));
    assert_eq!(add_tag, Some("new".to_string()));
}

#[test]
fn cli_tag_add_tag_and_add_many_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--add-tag",
        "java",
        "--add-many-tags",
        "python",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--add-tag and --add-many-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_tag_and_remove_many_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-tag",
        "apple",
        "--remove-many-tags",
        "banana",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-tag and --remove-many-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_tag_and_remove_all_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-tag",
        "apple",
        "--remove-all-tags",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-tag and --remove-all-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_many_tags_and_remove_all_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-many-tags",
        "apple",
        "--remove-all-tags",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-many-tags and --remove-all-tags must be mutually exclusive"
    );
}

// -----------------------------------------------------------------------
// CLI argument parsing — Create subcommand
// -----------------------------------------------------------------------

/// Helper: parse a `BugCommand::Create` from CLI tokens and return the variant.
fn parse_create_cmd(args: &[&str]) -> BugCommand {
    let full: Vec<&str> = std::iter::once("lpcli")
        .chain(std::iter::once("bug"))
        .chain(std::iter::once("create"))
        .chain(args.iter().copied())
        .collect();
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    match cli.command {
        Command::Bug(cmd) => cmd,
        _ => panic!("Expected Bug command"),
    }
}

#[test]
fn cli_create_bug_basic() {
    let cmd = parse_create_cmd(&[
        "--target",
        "ubuntu",
        "--package",
        "curl",
        "--title",
        "crash on boot",
        "--description",
        "detailed description",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "ubuntu");
    assert_eq!(package, Some("curl".to_string()));
    assert_eq!(title, "crash on boot");
    assert_eq!(description, "detailed description");
    assert!(attachment_file.is_none());
    assert!(attachment_url.is_none());
    assert!(attachment_description.is_none());
    assert!(attachment_type.is_none());
}

#[test]
fn cli_create_bug_with_attachment_file() {
    let cmd = parse_create_cmd(&[
        "-t",
        "launchpad",
        "-T",
        "new bug",
        "-d",
        "some issue",
        "-f",
        "/tmp/patch.diff",
        "--attachment-description",
        "fix for crash",
        "-a",
        "Patch",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "launchpad");
    assert!(package.is_none());
    assert_eq!(title, "new bug");
    assert_eq!(description, "some issue");
    assert_eq!(attachment_file, Some("/tmp/patch.diff".to_string()));
    assert!(attachment_url.is_none());
    assert_eq!(attachment_description, Some("fix for crash".to_string()));
    assert_eq!(attachment_type, Some("Patch".to_string()));
}

#[test]
fn cli_create_bug_with_attachment_url() {
    let cmd = parse_create_cmd(&[
        "-T",
        "new bug",
        "-d",
        "some issue",
        "-u",
        "https://example.com/log.txt",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "ubuntu"); // default value
    assert!(package.is_none());
    assert_eq!(title, "new bug");
    assert_eq!(description, "some issue");
    assert!(attachment_file.is_none());
    assert_eq!(
        attachment_url,
        Some("https://example.com/log.txt".to_string())
    );
    assert!(attachment_description.is_none());
    assert!(attachment_type.is_none());
}

#[test]
fn format_size_standalone_bytes() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1023), "1023 B");
}

#[test]
fn format_size_binary_prefixes() {
    assert_eq!(format_size(1_024), "1.0 KiB");
    assert_eq!(format_size(1_536), "1.5 KiB");
    assert_eq!(format_size(1_048_576), "1.0 MiB");
    assert_eq!(format_size(1_073_741_824), "1.0 GiB");
}

// -----------------------------------------------------------------------
// CLI argument parsing — AddTask with remote bug tracker
//
// Bug IDs (42) and remote bug IDs (12345) are arbitrary fake values used
// only to exercise argument parsing; no API calls are made in these tests.
// -----------------------------------------------------------------------

#[test]
fn cli_add_task_with_remote_bug_and_tracker() {
    let full = [
        "lpcli",
        "bug",
        "add-task",
        "--bug-id",
        "42",
        "--target",
        "debian",
        "--package",
        "arctica-greeter",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "12345",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::AddTask {
        bug_id,
        target,
        package,
        bug_tracker,
        remote_bug,
        ..
    }) = cli.command
    else {
        panic!("Expected AddTask");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target, "debian");
    assert_eq!(package.as_deref(), Some("arctica-greeter"));
    assert_eq!(bug_tracker.as_deref(), Some("debbugs"));
    assert_eq!(remote_bug.as_deref(), Some("12345"));
}

#[test]
fn cli_link_remote_full() {
    let full = [
        "lpcli",
        "bug",
        "link-remote",
        "--bug-id",
        "42",
        "--target",
        "debian",
        "--package",
        "arctica-greeter",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "12345",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::LinkRemote {
        bug_id,
        target,
        package,
        bug_tracker,
        remote_bug,
        series,
        task_url,
    }) = cli.command
    else {
        panic!("Expected LinkRemote");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target.as_deref(), Some("debian"));
    assert_eq!(package.as_deref(), Some("arctica-greeter"));
    assert_eq!(bug_tracker, "debbugs");
    assert_eq!(remote_bug, "12345");
    assert!(series.is_none());
    assert!(task_url.is_none());
}

#[test]
fn cli_link_remote_aliases() {
    for alias in &["link-remote", "link-task"] {
        let full = [
            "lpcli",
            "bug",
            alias,
            "--bug-id",
            "42",
            "--tracker",
            "debbugs",
            "--remote",
            "12345",
        ];
        let cli = Cli::try_parse_from(full).expect("CLI parsing failed for alias");
        let Command::Bug(BugCommand::LinkRemote {
            bug_id,
            bug_tracker,
            remote_bug,
            ..
        }) = cli.command
        else {
            panic!("Expected LinkRemote for alias {alias}");
        };
        assert_eq!(bug_id, 42);
        assert_eq!(bug_tracker, "debbugs");
        assert_eq!(remote_bug, "12345");
    }
}

#[test]
fn cli_unlink_remote() {
    let full = [
        "lpcli",
        "bug",
        "unlink-remote",
        "--bug-id",
        "42",
        "--target",
        "debian",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::UnlinkRemote { bug_id, target, .. }) = cli.command else {
        panic!("Expected UnlinkRemote");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target.as_deref(), Some("debian"));
}

#[test]
fn cli_watches() {
    let full = ["lpcli", "bug", "watches", "--bug-id", "42"];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::Watches { bug_id }) = cli.command else {
        panic!("Expected Watches");
    };
    assert_eq!(bug_id, 42);
}

#[test]
fn cli_remote_bug_trackers_filters_and_limits() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "remote-bug-trackers",
        "--limit",
        "2",
        "--search",
        "DebÜG",
    ])
    .expect("CLI parsing failed");
    let Command::Bug(BugCommand::RemoteBugTrackers { limit, search }) = cli.command else {
        panic!("Expected RemoteBugTrackers");
    };
    assert_eq!(limit, Some(2));
    assert_eq!(search.as_deref(), Some("DebÜG"));
}

#[test]
fn cli_remote_bug_trackers_accepts_short_options() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "remote-bug-trackers",
        "-l",
        "3",
        "-s",
        "debian",
    ])
    .expect("CLI parsing failed");
    let Command::Bug(BugCommand::RemoteBugTrackers { limit, search }) = cli.command else {
        panic!("Expected RemoteBugTrackers");
    };
    assert_eq!(limit, Some(3));
    assert_eq!(search.as_deref(), Some("debian"));
}

#[test]
fn cli_remote_bug_trackers_rejects_zero_limit() {
    let result = Cli::try_parse_from(["lpcli", "bug", "remote-bug-trackers", "--limit", "0"]);
    assert!(result.is_err(), "the limit must be positive");
}

#[test]
fn remote_link_failures_include_all_targets_and_missing_self_link() {
    let failures = vec![
        remote_link_failure("noble", "debbugs", "1150409", "Launchpad API error 500"),
        remote_link_failure("jammy", "debbugs", "1150409", "new task has no self_link"),
    ];
    let err = remote_link_failure_error(42, &failures)
        .expect("remote-link failures should become a command error");
    let message = err.to_string();

    assert!(message.contains("bug #42"));
    assert!(message.contains("task creation may already have succeeded"));
    assert!(message.contains("series 'noble'"));
    assert!(message.contains("series 'jammy'"));
    assert!(message.contains("new task has no self_link"));
}

#[test]
fn no_remote_link_failures_preserve_success() {
    assert!(remote_link_failure_error(42, &[]).is_none());
}

// -----------------------------------------------------------------------
// find_task_to_link unit tests
// -----------------------------------------------------------------------

fn make_test_task(
    self_link: &str,
    target_link: &str,
    display_name: &str,
    watch_link: Option<&str>,
) -> bugs::BugTask {
    bugs::BugTask {
        self_link: Some(self_link.to_string()),
        bug_link: Some("https://api.launchpad.net/devel/bugs/42".to_string()),
        title: Some("Test bug".to_string()),
        status: Some("New".to_string()),
        importance: Some("Undecided".to_string()),
        assignee_link: None,
        date_created: None,
        bug_target_display_name: Some(display_name.to_string()),
        target_link: Some(target_link.to_string()),
        bug_watch_link: watch_link.map(|w| w.to_string()),
    }
}

#[test]
fn find_task_to_link_by_task_url() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        task_url: Some("https://api.launchpad.net/devel/debian/+source/curl/+bug/42".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should find task by url");

    assert_eq!(
        found.self_link.as_deref(),
        Some("https://api.launchpad.net/devel/debian/+source/curl/+bug/42")
    );
}

#[test]
fn find_task_to_link_by_target_and_distro() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("debian".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should find debian task");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Debian)")
    );
}

#[test]
fn find_task_to_link_prefers_series_less() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
        "curl (Ubuntu Noble)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("curl".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should prefer series-less task");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Ubuntu)")
    );
}

#[test]
fn find_task_to_link_with_series() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
        "curl (Ubuntu Noble)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("curl".to_string()),
        series: Some("noble".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should match noble series");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Ubuntu Noble)")
    );
}

#[test]
fn find_task_to_link_ambiguous_fails() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        package: Some("curl".to_string()),
        ..Default::default()
    };
    let res = find_task_to_link(&tasks, 42, &selector);
    assert!(res.is_err());
}

#[test]
fn find_task_to_link_not_found_fails() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let tasks = vec![t1];
    let selector = TaskLinkSelector {
        target: Some("debian".to_string()),
        ..Default::default()
    };
    let res = find_task_to_link(&tasks, 42, &selector);
    assert!(res.is_err());
}
