//! Phase 3 tests: Rich snapshots for bug commands, semantic ANSI colors,
//! normalized usernames/dates, and `$PAGER` / `--no-pager` support.

use serde_json::{Value, json};

use crate::output::Renderer as _;

use super::*;

fn bug_command(argv: &[&str]) -> BugCommand {
    let cli = Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    validate_cli(&cli).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    let Command::Bug(command) = cli.command else {
        panic!("{argv:?} is not a bug command");
    };
    command
}

async fn run_bug(server: &mockito::Server, argv: &[&str]) -> lpcli::error::Result<BugOutcome> {
    let client = mock_read_client(server, true);
    bug_outcome(&client, bug_command(argv)).await
}

fn render_rich(outcome: &BugOutcome, color: bool) -> String {
    outcome.render(&Output::fixed(OutputMode::Rich, color, 100), false)
}

fn render_plain(outcome: &BugOutcome) -> String {
    outcome.render(&Output::fixed(OutputMode::Plain, false, 100), false)
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

async fn mock_get(server: &mut mockito::Server, path: &str, body: &Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

async fn mock_redirect(server: &mut mockito::Server, path: &str, location: &str) -> mockito::Mock {
    server
        .mock("GET", path)
        .with_status(303)
        .with_header("location", location)
        .create_async()
        .await
}

// ---------------------------------------------------------------------------
// Rich snapshots for ported bug read commands
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_show_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _bug = mock_get(
        &mut server,
        "/bugs/42",
        &json!({
            "id": 42,
            "title": "Daemon fails when XDG_RUNTIME_DIR is unset",
            "description": "Starts with exit code 1 unless runtime dir exists.",
            "tags": ["packaging", "regression"],
            "users_affected_count": 14,
            "web_link": "https://bugs.launchpad.net/bugs/42",
            "self_link": format!("{base}/bugs/42"),
        }),
    )
    .await;
    let _tasks = mock_get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            json!({
                "bug_target_display_name": "curl (Ubuntu)",
                "target_link": "https://api.launchpad.net/devel/ubuntu/+source/curl",
                "status": "Confirmed",
                "importance": "High",
                "assignee_link": format!("{base}/~jdoe"),
                "self_link": format!("{base}/ubuntu/+source/curl/+bug/42"),
            }),
            json!({
                "bug_target_display_name": "curl (Ubuntu Noble)",
                "target_link": "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
                "status": "In Progress",
                "importance": "Critical",
                "bug_watch_link": format!("{base}/bugs/42/+watch/9001"),
                "self_link": format!("{base}/ubuntu/noble/+source/curl/+bug/42"),
            }),
        ]),
    )
    .await;
    let _attachments = mock_get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![json!({
            "title": "fix.patch",
            "type": "Patch",
            "data_link": format!("{base}/attachments/1/data"),
            "self_link": format!("{base}/bugs/42/+attachment/1"),
        })]),
    )
    .await;
    let _dl = mock_redirect(
        &mut server,
        "/attachments/1/data",
        "https://launchpadlibrarian.net/999/fix.patch",
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "show", "--bug-id", "42"])
        .await
        .unwrap();
    insta::assert_snapshot!("bug_show_rich", render_rich(&outcome, false));
}

#[tokio::test]
async fn bug_tasks_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _tasks = mock_get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            json!({
                "bug_target_display_name": "curl (Ubuntu)",
                "target_link": "https://api.launchpad.net/devel/ubuntu/+source/curl",
                "status": "Fix Released",
                "importance": "Medium",
                "assignee_link": format!("{base}/~alice"),
                "self_link": format!("{base}/ubuntu/+source/curl/+bug/42"),
            }),
            json!({
                "bug_target_display_name": "curl (Ubuntu Jammy)",
                "target_link": "https://api.launchpad.net/devel/ubuntu/jammy/+source/curl",
                "status": "Invalid",
                "importance": "Low",
                "self_link": format!("{base}/ubuntu/jammy/+source/curl/+bug/42"),
            }),
        ]),
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "tasks", "--bug-id", "42"])
        .await
        .unwrap();
    insta::assert_snapshot!("bug_tasks_rich", render_rich(&outcome, false));
}

#[tokio::test]
async fn bug_comments_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let msg_link = format!("{base}/bugs/42/messages/1");
    let _comments = mock_get(
        &mut server,
        "/bugs/42/messages",
        &collection(vec![
            json!({
                "index": 0,
                "owner_link": format!("{base}/~reporter"),
                "date_created": "2026-11-15T08:00:00+00:00",
                "content": "Initial report with logs attached.",
                "self_link": format!("{base}/bugs/42/messages/0"),
            }),
            json!({
                "index": 1,
                "owner_link": format!("{base}/~jdoe"),
                "date_created": "2026-11-16T09:15:30+00:00",
                "content": "Reproduced cleanly on noble.",
                "self_link": msg_link,
            }),
        ]),
    )
    .await;
    let _attachments = mock_get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![json!({
            "title": "trace.log",
            "type": "Unspecified",
            "message_link": msg_link,
            "data_link": format!("{base}/attachments/2/data"),
            "self_link": format!("{base}/bugs/42/+attachment/2"),
        })]),
    )
    .await;
    let _dl = mock_redirect(
        &mut server,
        "/attachments/2/data",
        "https://launchpadlibrarian.net/1000/trace.log",
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "comments", "--bug-id", "42"])
        .await
        .unwrap();
    insta::assert_snapshot!("bug_comments_rich", render_rich(&outcome, false));
}

#[tokio::test]
async fn bug_subscriptions_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _subs = mock_get(
        &mut server,
        "/bugs/42/subscriptions",
        &collection(vec![
            json!({
                "person_link": format!("{base}/~jdoe"),
                "subscribed_by_link": format!("{base}/~alice"),
                "date_created": "2026-11-14T12:00:00+00:00",
                "self_link": format!("{base}/bugs/42/+subscription/jdoe"),
            }),
            json!({
                "person_link": format!("{base}/~ubuntu-server"),
                "subscribed_by_link": format!("{base}/~ubuntu-server"),
                "date_created": "2026-11-15T18:30:00+00:00",
                "self_link": format!("{base}/bugs/42/+subscription/ubuntu-server"),
            }),
        ]),
    )
    .await;

    let outcome = run_bug(
        &server,
        &["lpcli", "bug", "subscriptions", "--bug-id", "42"],
    )
    .await
    .unwrap();
    insta::assert_snapshot!("bug_subscriptions_rich", render_rich(&outcome, false));
}

#[tokio::test]
async fn bug_attachments_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _attachments = mock_get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![
            json!({
                "title": "debdiff.patch",
                "type": "Patch",
                "data_link": format!("{base}/attachments/10/data"),
                "self_link": format!("{base}/bugs/42/+attachment/10"),
            }),
            json!({
                "title": "journal.txt",
                "type": "Unspecified",
                "data_link": format!("{base}/attachments/11/data"),
                "self_link": format!("{base}/bugs/42/+attachment/11"),
            }),
        ]),
    )
    .await;
    let _dl1 = mock_redirect(
        &mut server,
        "/attachments/10/data",
        "https://launchpadlibrarian.net/2001/debdiff.patch",
    )
    .await;
    let _dl2 = mock_redirect(
        &mut server,
        "/attachments/11/data",
        "https://launchpadlibrarian.net/2002/journal.txt",
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "attachments", "--bug-id", "42"])
        .await
        .unwrap();
    insta::assert_snapshot!("bug_attachments_rich", render_rich(&outcome, false));
}

#[tokio::test]
async fn bug_watches_rich_matches_snapshot() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _watches = mock_get(
        &mut server,
        "/bugs/42/bug_watches",
        &collection(vec![
            json!({
                "bug_tracker_link": format!("{base}/bugs/bugtrackers/github-curl"),
                "remote_bug": "1234",
                "remote_status": "open",
                "url": "https://github.com/curl/curl/issues/1234",
                "self_link": format!("{base}/bugs/42/+watch/9001"),
            }),
            json!({
                "bug_tracker_link": format!("{base}/bugs/bugtrackers/debbugs"),
                "remote_bug": "987654",
                "remote_status": "done",
                "url": "https://bugs.debian.org/987654",
                "self_link": format!("{base}/bugs/42/+watch/9002"),
            }),
        ]),
    )
    .await;
    let _tasks = mock_get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![json!({
            "bug_target_display_name": "curl (Ubuntu)",
            "target_link": "https://api.launchpad.net/devel/ubuntu/+source/curl",
            "status": "Confirmed",
            "importance": "High",
            "bug_watch_link": format!("{base}/bugs/42/+watch/9001"),
            "self_link": format!("{base}/ubuntu/+source/curl/+bug/42"),
        })]),
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "watches", "--bug-id", "42"])
        .await
        .unwrap();
    insta::assert_snapshot!("bug_watches_rich", render_rich(&outcome, false));
}

// ---------------------------------------------------------------------------
// Semantic ANSI colors, Span::Name, and Rich warnings
// ---------------------------------------------------------------------------

#[test]
fn rich_color_applies_semantic_palette_to_fields_and_tables() {
    let bold = "\x1b[1m";
    let ok = "\x1b[1m\x1b[32m";
    let warn = "\x1b[1m\x1b[33m";
    let err = "\x1b[1m\x1b[31m";
    let name = "\x1b[1m\x1b[36m";
    let reset = "\x1b[0m";

    let report = Report {
        blocks: vec![
            Block::Field("Status", "Fix Released".into()),
            Block::Field("Lifecycle", "In Progress".into()),
            Block::Field("Impl", "Invalid".into()),
            Block::Field("Upload Status", "New".into()),
            Block::Field("Importance", "Critical".into()),
            Block::Field("Priority", "High".into()),
            Block::Field("Tags", "packaging, regression".into()),
            Block::Field("Author", "alice".into()),
            Block::Field("Assignee", "none".into()),
            Block::Field("Subscribed By", "unknown".into()),
            Block::Field("Member", "—".into()),
            Block::Table {
                records: Records {
                    columns: vec![
                        Column::new("Status", "status"),
                        Column::new("Importance", "importance"),
                        Column::new("Assignee", "assignee"),
                        Column::new("Person", "person"),
                    ],
                    rows: vec![
                        vec![
                            Some("Confirmed".into()),
                            Some("Medium".into()),
                            Some("bob".into()),
                            Some("carol".into()),
                        ],
                        vec![
                            Some("Rejected".into()),
                            Some("Low".into()),
                            None,
                            Some("ubuntu-dev".into()),
                        ],
                    ],
                },
                noun: Noun {
                    singular: "item",
                    plural: "items",
                },
                empty: "None.".into(),
                hint: None,
            },
        ],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };

    let colored = Output::fixed(OutputMode::Rich, true, 120).render(&report);
    for expected in [
        format!("{ok}Fix Released{reset}"),
        format!("{warn}In Progress{reset}"),
        format!("{err}Invalid{reset}"),
        format!("{name}New{reset}"),
        format!("{err}Critical{reset}"),
        format!("{warn}High{reset}"),
        format!("{name}packaging, regression{reset}"),
        format!("{name}alice{reset}"),
        format!("{warn}Confirmed{reset}"),
        format!("{warn}Medium{reset}"),
        format!("{name}bob{reset}"),
        format!("{name}carol{reset}"),
        format!("{err}Rejected{reset}"),
        format!("{name}Low{reset}"),
        format!("{name}ubuntu-dev{reset}"),
    ] {
        assert!(
            colored.contains(&expected),
            "missing {expected:?} in:\n{colored:?}"
        );
    }

    // Placeholders stay uncolored.
    for placeholder in ["none", "unknown", "—"] {
        assert!(
            !colored.contains(&format!("{name}{placeholder}")),
            "placeholder {placeholder:?} unexpectedly colored in:\n{colored:?}"
        );
    }

    // Header row labels are bolded and stay uncolored by value styles.
    assert!(
        colored.contains(&format!("{bold}Status{reset}")),
        "table header should be bold:\n{colored:?}"
    );
    assert!(
        colored.contains("─┼─"),
        "expected lightweight borderless header rule:\n{colored:?}"
    );
    assert!(!colored.contains(&format!("{ok}Status")));
    assert!(!colored.contains(&format!("{name}Assignee")));

    // color == false emits zero ANSI escape sequences.
    let uncolored = Output::fixed(OutputMode::Rich, false, 120).render(&report);
    assert!(!uncolored.contains('\x1b'), "{uncolored}");
}

#[tokio::test]
async fn bug_mutations_color_names_and_render_warnings_in_rich_mode() {
    let name = "\x1b[1m\x1b[36m";
    let reset = "\x1b[0m";

    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _tasks = mock_get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![json!({
            "bug_target_display_name": "curl (Ubuntu)",
            "target_link": format!("{base}/ubuntu/+source/curl"),
            "self_link": format!("{base}/ubuntu/+source/curl/+bug/42"),
        })]),
    )
    .await;
    let _patch_task = server
        .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "bug_target_display_name": "curl (Ubuntu)",
                "assignee_link": format!("{base}/~jdoe"),
                "self_link": format!("{base}/ubuntu/+source/curl/+bug/42"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let _sub = server
        .mock("POST", "/bugs/42")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "person_link": format!("{base}/~jdoe"),
                "self_link": format!("{base}/bugs/42/+subscription/jdoe"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let _bug = mock_get(
        &mut server,
        "/bugs/42",
        &json!({
            "id": 42,
            "title": "Test bug",
            "tags": ["old"],
            "self_link": format!("{base}/bugs/42"),
        }),
    )
    .await;
    let _patch_bug = server
        .mock("PATCH", "/bugs/42")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "id": 42,
                "title": "Test bug",
                "tags": ["old", "focal"],
                "self_link": format!("{base}/bugs/42"),
            })
            .to_string(),
        )
        .create_async()
        .await;

    let assignee = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "set-assignee",
            "--bug-id",
            "42",
            "--name",
            "jdoe",
            "-t",
            "curl",
            "--all-series",
        ],
    )
    .await
    .unwrap();
    let assignee_color = render_rich(&assignee, true);
    assert!(
        assignee_color.contains(&format!("{name}~jdoe{reset}")),
        "{assignee_color:?}"
    );
    assert!(!render_rich(&assignee, false).contains('\x1b'));

    let sub = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "subscribe",
            "--bug-id",
            "42",
            "--name",
            "jdoe",
        ],
    )
    .await
    .unwrap();
    assert!(render_rich(&sub, true).contains(&format!("{name}~jdoe{reset}")));

    let tag = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "tag",
            "--bug-id",
            "42",
            "--add-tag",
            "focal",
        ],
    )
    .await
    .unwrap();
    assert!(render_rich(&tag, true).contains(&format!("{name}old, focal{reset}")));

    // BugOutcome::render_warnings uses Output's Rich warning prefix when color=true.
    let outcome = BugOutcome {
        body: BugBody::Empty,
        warnings: vec!["task update failed for 'curl'".into()],
        quiet: None,
        error: None,
    };
    let colored_warn = outcome.render_warnings(&Output::fixed(OutputMode::Rich, true, 100));
    assert!(
        colored_warn.contains("\x1b[1m\x1b[33mwarning:\x1b[0m task update failed for 'curl'"),
        "{colored_warn:?}"
    );
    let uncolored_warn = outcome.render_warnings(&Output::fixed(OutputMode::Rich, false, 100));
    assert_eq!(uncolored_warn, "warning: task update failed for 'curl'\n");
}

// ---------------------------------------------------------------------------
// Username extraction and timestamp normalization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_and_git_comments_normalize_usernames_and_dates() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let _comments = mock_get(
        &mut server,
        "/bugs/7/messages",
        &collection(vec![json!({
            "index": 1,
            "owner_link": format!("{base}/~jdoe"),
            "date_created": "2026-11-16T09:15:30+00:00",
            "content": "Looks good.",
            "self_link": format!("{base}/bugs/7/messages/1"),
        })]),
    )
    .await;
    let _attachments = mock_get(&mut server, "/bugs/7/attachments", &collection(vec![])).await;

    let bug_out = run_bug(&server, &["lpcli", "bug", "comments", "--bug-id", "7"])
        .await
        .unwrap();
    let rich_bug = render_rich(&bug_out, false);
    assert!(
        rich_bug.contains(" #1 ┆ jdoe ┆ 2026-11-16 09:15 "),
        "{rich_bug}"
    );
    assert!(rich_bug.contains("1 comment found.\n\n"), "{rich_bug}");
    let plain_bug = render_plain(&bug_out);
    assert!(plain_bug.contains("Author: jdoe"), "{plain_bug}");
    assert!(plain_bug.contains("Date: 2026-11-16 09:15"), "{plain_bug}");
    for rendered in [rich_bug, plain_bug] {
        assert!(!rendered.contains(&base), "{rendered}");
        assert!(!rendered.contains("~jdoe"), "{rendered}");
        assert!(!rendered.contains("T09:15:30"), "{rendered}");
    }

    let review_raw = collection(vec![json!({
        "id": 5,
        "author_link": "https://api.launchpad.net/devel/~alice",
        "date_created": "2026-11-16T14:22:10+00:00",
        "vote": "Approve",
        "content": "Ship it",
    })]);
    let inline_raw = json!([{
        "line_number": "12",
        "person": "~reviewer",
        "date": "2026-11-16T09:15:30+00:00",
        "text": "Nit on naming",
    }]);
    let git_comments = GitComments {
        comments: Raw {
            value: serde_json::from_value(review_raw["entries"].clone()).unwrap(),
            raw: review_raw,
        },
        inline: Raw {
            value: serde_json::from_value(inline_raw.clone()).unwrap(),
            raw: inline_raw,
        },
        diff_map: Vec::new(),
    };
    let report = git_comments_report(&git_comments);
    let rich_git = Output::fixed(OutputMode::Rich, false, 100).render(&report);
    assert!(
        rich_git.contains("1 review comment found.\n\n"),
        "{rich_git}"
    );
    assert!(
        rich_git.contains(" #5 ┆ alice") && rich_git.contains("┆ 2026-11-16 14:22 "),
        "{rich_git}"
    );
    assert!(rich_git.contains("Vote: Approve"), "{rich_git}");
    assert!(
        rich_git.contains("1 inline comment found.\n\n"),
        "{rich_git}"
    );
    assert!(
        rich_git.contains(" diff line 12 ┆ reviewer") && rich_git.contains("┆ 2026-11-16 09:15 "),
        "{rich_git}"
    );
    assert!(rich_git.contains("Nit on naming"), "{rich_git}");

    let plain_git = Output::fixed(OutputMode::Plain, false, 100).render(&report);
    assert!(plain_git.contains("Comment: #5"), "{plain_git}");
    assert!(plain_git.contains("Author: alice"), "{plain_git}");
    assert!(plain_git.contains("Date: 2026-11-16 14:22"), "{plain_git}");
    assert!(plain_git.contains("Vote: Approve"), "{plain_git}");
    assert!(plain_git.contains("Location: diff line 12"), "{plain_git}");
    assert!(plain_git.contains("Author: reviewer"), "{plain_git}");
    assert!(plain_git.contains("Date: 2026-11-16 09:15"), "{plain_git}");

    for rendered in [rich_git, plain_git] {
        assert!(!rendered.contains("~reviewer"), "{rendered}");
        assert!(
            !rendered.contains("https://api.launchpad.net"),
            "{rendered}"
        );
        assert!(!rendered.contains("2026-11-16T"), "{rendered}");
    }
}

// ---------------------------------------------------------------------------
// Pager support (--no-pager, resolve_pager, should_page, write_with_pager)
// ---------------------------------------------------------------------------

#[test]
fn cli_no_pager_flag_and_pager_resolution() {
    let cli =
        Cli::try_parse_from(["lpcli", "--no-pager", "bug", "show", "--bug-id", "42"]).unwrap();
    assert!(cli.no_pager);
    validate_cli(&cli).unwrap();

    let env_less = output::Environment {
        pager: Some("less -FRX".into()),
        term: Some("xterm-256color".into()),
        ..Default::default()
    };

    // Pager is active for rich/plain on a non-dumb TTY.
    assert_eq!(
        output::resolve_pager(false, OutputMode::Rich, &env_less, true).as_deref(),
        Some("less -FRX")
    );
    assert_eq!(
        output::resolve_pager(false, OutputMode::Plain, &env_less, true).as_deref(),
        Some("less -FRX")
    );

    // Disabled by --no-pager, non-TTY, TERM=dumb, or structured modes.
    assert_eq!(
        output::resolve_pager(true, OutputMode::Rich, &env_less, true),
        None
    );
    assert_eq!(
        output::resolve_pager(false, OutputMode::Rich, &env_less, false),
        None
    );
    let env_dumb = output::Environment {
        term: Some("dumb".into()),
        ..env_less.clone()
    };
    assert_eq!(
        output::resolve_pager(false, OutputMode::Rich, &env_dumb, true),
        None
    );
    assert_eq!(
        output::resolve_pager(false, OutputMode::Json, &env_less, true),
        None
    );
    assert_eq!(
        output::resolve_pager(false, OutputMode::Tsv, &env_less, true),
        None
    );

    // Disabled when $PAGER is unset, blank, or "cat".
    for val in [None, Some(""), Some("   "), Some("cat")] {
        let env_custom = output::Environment {
            pager: val.map(str::to_string),
            ..env_less.clone()
        };
        assert_eq!(
            output::resolve_pager(false, OutputMode::Rich, &env_custom, true),
            None
        );
    }

    // resolve_command_output wires --no-pager and $PAGER onto Output.
    let settings = config::AppConfig::default();
    let default_cli = Cli::try_parse_from(["lpcli", "bug", "show", "--bug-id", "42"]).unwrap();
    let resolved =
        resolve_command_output(&default_cli, &settings, &env_less, (true, false)).unwrap();
    assert_eq!(resolved.pager(), Some("less -FRX"));

    let resolved_disabled =
        resolve_command_output(&cli, &settings, &env_less, (true, false)).unwrap();
    assert_eq!(resolved_disabled.pager(), None);
}

#[test]
fn should_page_and_write_with_pager_behave_as_documented() {
    let two_lines = "line 1\nline 2\n";
    let four_lines = "line 1\nline 2\nline 3\nline 4\n";

    assert!(!output::should_page(Some("less -FRX"), two_lines, 3));
    assert!(output::should_page(Some("less -FRX"), four_lines, 3));
    assert!(!output::should_page(Some("cat"), four_lines, 3));
    assert!(!output::should_page(None, four_lines, 3));

    // When PAGER is disabled or output fits in max_lines, pager is not spawned.
    assert!(
        !output::write_with_pager("", Some("false"), 3).unwrap(),
        "empty text should not invoke pager"
    );
    assert!(
        !output::write_with_pager("", Some("cat"), 0).unwrap(),
        "cat counts as disabled"
    );

    // When lines > max_lines, write_with_pager pipes the text to the pager process.
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("paged.txt");
    let pager_cmd = format!("sh -c 'cat > \"{}\"'", dest.display());
    let paged = output::write_with_pager(four_lines, Some(&pager_cmd), 3).unwrap();
    assert!(paged, "output longer than max_lines must be piped to pager");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), four_lines);

    // Pager disabling also handles `/usr/bin/cat` and `sudo` contexts.
    for cmd in ["/bin/cat", "/usr/bin/cat", "cat -u"] {
        assert!(!output::should_page(Some(cmd), four_lines, 3), "{cmd}");
    }
    let sudo_env = output::Environment {
        pager: Some("less -FRX".into()),
        term: Some("xterm-256color".into()),
        sudo: true,
        ..Default::default()
    };
    assert_eq!(
        output::resolve_pager(false, OutputMode::Rich, &sudo_env, true),
        None
    );

    // Failing pager returns Err instead of silently losing output.
    assert!(output::write_with_pager(four_lines, Some("sh -c 'exit 2'"), 3).is_err());
}

#[test]
fn table_and_field_coloring_handles_delimiters_wrapping_stacking_and_no_color_override() {
    let ok = "\x1b[1m\x1b[32m";
    let name = "\x1b[1m\x1b[36m";
    let reset = "\x1b[0m";

    // 1. Literal `'┆'` inside a cell must not break column alignment or colorization.
    let bar_report = Report {
        blocks: vec![Block::Table {
            records: Records {
                columns: vec![
                    Column::new("Title", "title"),
                    Column::new("Status", "status"),
                ],
                rows: vec![vec![
                    Some("left ┆ right".into()),
                    Some("Fix Released".into()),
                ]],
            },
            noun: Noun {
                singular: "bug",
                plural: "bugs",
            },
            empty: "None.".into(),
            hint: None,
        }],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };
    let colored_bar = Output::fixed(OutputMode::Rich, true, 120).render(&bar_report);
    assert!(colored_bar.contains("left ┆ right"), "{colored_bar:?}");
    assert!(
        colored_bar.contains(&format!("{ok}Fix Released{reset}")),
        "{colored_bar:?}"
    );

    // 2. Narrow tables wrapping multi-word statuses paint every wrapped fragment.
    let wrap_report = Report {
        blocks: vec![Block::Table {
            records: Records {
                columns: vec![Column::new("Id", "id"), Column::new("Status", "status")],
                rows: vec![vec![Some("1".into()), Some("Fix Released".into())]],
            },
            noun: Noun {
                singular: "bug",
                plural: "bugs",
            },
            empty: "None.".into(),
            hint: None,
        }],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };
    let wrapped = Output::fixed(OutputMode::Rich, true, 16).render(&wrap_report);
    assert!(wrapped.contains(&format!("{ok}Fix{reset}")), "{wrapped:?}");
    assert!(
        wrapped.contains(&format!("{ok}Released{reset}")),
        "{wrapped:?}"
    );

    // 3. Stacked tables color only real semantic columns, not continuation lines containing `: `.
    let stacked_report = Report {
        blocks: vec![Block::Table {
            records: Records {
                columns: vec![
                    Column::new("Summary", "summary"),
                    Column::new("Status", "status"),
                ],
                rows: vec![vec![
                    Some("first line\nAuthor: fake".into()),
                    Some("Fix Released".into()),
                ]],
            },
            noun: Noun {
                singular: "item",
                plural: "items",
            },
            empty: "None.".into(),
            hint: None,
        }],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };
    let stacked = Output::fixed(OutputMode::Rich, true, 5).render(&stacked_report);
    assert!(
        stacked.contains(&format!("Status: {ok}Fix Released{reset}")),
        "{stacked:?}"
    );
    assert!(
        !stacked.contains(&format!("{name}fake{reset}")),
        "continuation line must not be mis-colored: {stacked:?}"
    );

    // 4. Multi-line fields paint each non-empty line independently.
    let multiline_field = Report {
        blocks: vec![Block::Field("Tags", "tag-a\ntag-b".into())],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };
    let field_rendered = Output::fixed(OutputMode::Rich, true, 80).render(&multiline_field);
    assert_eq!(
        field_rendered,
        format!("  Tags: {name}tag-a{reset}\n{name}tag-b{reset}\n")
    );

    // 5. ColorChoice::Always overrides NO_COLOR consistently for tables via anstyle.
    let env_no_color = output::Environment {
        no_color: Some("1".into()),
        term: Some("xterm-256color".into()),
        ..Default::default()
    };
    let always_out = Output::resolve(output::Settings {
        format: None,
        color: ColorChoice::Always,
        config_format: None,
        accessible: false,
        display: Display::from_width(120),
        json_output: true,
        tsv_output: true,
        fields: None,
        env: &env_no_color,
        stdout_terminal: false,
        stderr_terminal: false,
    })
    .unwrap();
    assert!(
        always_out
            .render(&bar_report)
            .contains(&format!("{ok}Fix Released{reset}"))
    );
}

fn comment_report(entries: Vec<CommentEntry>, empty: &str) -> Report {
    Report {
        blocks: vec![Block::Comments {
            entries,
            noun: Noun {
                singular: "comment",
                plural: "comments",
            },
            empty: empty.to_string(),
        }],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    }
}

#[test]
fn bug_comments_rich_adapts_to_widths_and_preserves_text() {
    let long_body = "Investigated the failure carefully with clean build environment and reproduced the exact crash path";
    let report = comment_report(
        vec![
            CommentEntry {
                label_field: "Comment",
                label: "#0".into(),
                author: Some("reporter".into()),
                date: Some(CommentDate::Text("2026-11-15 08:00".into())),
                details: Vec::new(),
                content_field: "Comment",
                content: "Initial short note.".into(),
                attachments: Vec::new(),
            },
            CommentEntry {
                label_field: "Comment",
                label: "#1".into(),
                author: Some("ubuntu-developer".into()),
                date: Some(CommentDate::Text("2026-11-16 09:15".into())),
                details: Vec::new(),
                content_field: "Comment",
                content: long_body.into(),
                attachments: vec![CommentAttachment {
                    title: "trace.log".into(),
                    kind: "Patch".into(),
                    url: Some("https://launchpadlibrarian.net/1000/trace.log".into()),
                }],
            },
        ],
        "No comments found for bug #42.",
    );

    // Wide (140): fits naturally on a single card width without stretching to 140 columns.
    let wide = Output::fixed(OutputMode::Rich, false, 140).render(&report);
    let wide_max = wide
        .lines()
        .filter(|l| l.starts_with('─') && l.contains('┬'))
        .map(|l| l.chars().count())
        .max()
        .unwrap();
    assert!((90..=140).contains(&wide_max), "{wide}");
    assert!(wide.contains(long_body), "{wide}");
    assert!(wide.starts_with("2 comments found.\n\n"), "{wide}");
    assert!(!wide.contains('┌'), "{wide}");

    // Medium (60): keeps 3-column header + uniform header/body box width <= 60.
    let medium = Output::fixed(OutputMode::Rich, false, 60).render(&report);
    assert!(medium.contains('┬'), "{medium}");
    assert!(medium.contains('┴'), "{medium}");
    let rule_widths: Vec<usize> = medium
        .lines()
        .filter(|l| !l.is_empty() && l.chars().all(|c| matches!(c, '─' | '┬' | '┴' | '╌')))
        .map(|l| l.chars().count())
        .collect();
    assert!(!rule_widths.is_empty());
    assert!(
        rule_widths.iter().all(|&w| w == 60),
        "expected uniform 60-col card rules, got {rule_widths:?}:\n{medium}"
    );
    for word in long_body.split_whitespace() {
        assert!(
            medium.contains(word),
            "missing {word} at width 60:\n{medium}"
        );
    }

    // Narrow (30): falls back cleanly to 1-column card header while preserving every word.
    let narrow = Output::fixed(OutputMode::Rich, false, 30).render(&report);
    assert!(
        !narrow.contains('┴'),
        "expected 1-col header fallback:\n{narrow}"
    );
    assert!(narrow.contains(" · "), "{narrow}");
    for line in narrow.lines() {
        assert!(
            line.chars().count() <= 30,
            "line exceeded width 30 ({}): {line:?}\n{narrow}",
            line.chars().count()
        );
    }
    for word in long_body.split_whitespace() {
        assert!(
            narrow.contains(word),
            "missing {word} at width 30:\n{narrow}"
        );
    }

    // Extreme narrow (5): uses unboxed stacked rows matching Display::is_stacked(3).
    let stacked = Output::fixed(OutputMode::Rich, false, 5).render(&report);
    assert!(!stacked.contains('┬'), "{stacked}");
    assert!(stacked.starts_with("2 comments found.\n\n"), "{stacked}");
    assert!(
        stacked.contains("Comment: #0\nAuthor: reporter\n"),
        "{stacked}"
    );
    assert!(
        stacked.contains("Attachment: trace.log [Patch]"),
        "{stacked}"
    );
}

#[test]
fn bug_comments_rich_colors_and_edge_cases() {
    let bold = "\x1b[1m";
    let name = "\x1b[1m\x1b[36m";
    let reset = "\x1b[0m";

    // Empty comments list uses the clear empty sentence.
    let empty = comment_report(Vec::new(), "No comments found for bug #42.");
    assert_eq!(
        Output::fixed(OutputMode::Rich, true, 100).render(&empty),
        "No comments found for bug #42.\n"
    );

    // Rich semantic colors + edge cases: literal `┆`, body prose `"Attachment: ..."`,
    // CRLF vs bare `\r`, bidi override, multi-paragraph, multiple attachments,
    // attachment-only comment, missing metadata, and empty body without attachments.
    let report = comment_report(
        vec![
            CommentEntry {
                label_field: "Comment",
                label: "#0".into(),
                author: Some("alice ┆ team".into()),
                date: Some(CommentDate::Text("2026-11-15 08:00".into())),
                details: Vec::new(),
                content_field: "Comment",
                content: "First para\r\n\r\nAttachment: prose mention\r\u{202E}bad".into(),
                attachments: vec![
                    CommentAttachment {
                        title: "first.patch".into(),
                        kind: "Patch".into(),
                        url: Some("https://example.invalid/1".into()),
                    },
                    CommentAttachment {
                        title: "second.log".into(),
                        kind: "Unspecified".into(),
                        url: None,
                    },
                ],
            },
            CommentEntry {
                label_field: "Comment",
                label: "#1".into(),
                author: None,
                date: None,
                details: Vec::new(),
                content_field: "Comment",
                content: "   \n\t ".into(),
                attachments: vec![CommentAttachment {
                    title: "only.diff".into(),
                    kind: "Patch".into(),
                    url: Some("https://example.invalid/2".into()),
                }],
            },
            CommentEntry {
                label_field: "Comment",
                label: "#2".into(),
                author: Some("日本語user".into()),
                date: Some(CommentDate::Text("2026-11-16 10:00".into())),
                details: Vec::new(),
                content_field: "Comment",
                content: String::new(),
                attachments: Vec::new(),
            },
        ],
        "No comments.",
    );

    let colored = Output::fixed(OutputMode::Rich, true, 100).render(&report);
    assert!(colored.starts_with("3 comments found.\n\n"), "{colored:?}");
    assert!(colored.contains(&format!("{bold}#0{reset}")), "{colored:?}");
    assert!(
        colored.contains(&format!("{name}alice ┆ team{reset}")),
        "{colored:?}"
    );
    // Missing author (`none`) is not colored as a username.
    assert!(
        !colored.contains(&format!("{name}none{reset}")),
        "{colored:?}"
    );
    // Body prose containing `"Attachment:"` must NOT be bolded; only real attachments are.
    assert!(
        colored.contains(" Attachment: prose mention"),
        "body prose must not be bolded:\n{colored:?}"
    );
    assert_eq!(
        colored
            .matches(&format!("{bold}Attachment:{reset}"))
            .count(),
        3,
        "expected exactly the 3 real attachment prefixes bolded:\n{colored:?}"
    );
    // CRLF is normalized without replacement chars; bare CR and bidi override are neutralized.
    assert!(!colored.contains('\u{202E}'), "{colored:?}");
    assert!(!colored.contains('\r'), "{colored:?}");
    assert!(colored.contains("First para"), "{colored:?}");
    assert!(colored.contains("\u{FFFD}\u{FFFD}bad"), "{colored:?}");

    // Uncolored check: verify CJK header and body border widths are identical,
    // attachment-only comment renders directly below `──┴──` without an empty row,
    // and empty comment without attachments renders `none`.
    let plain_rich = Output::fixed(OutputMode::Rich, false, 100).render(&report);
    let top_widths: Vec<usize> = plain_rich
        .lines()
        .filter(|l| l.starts_with('─') && l.contains('┬'))
        .map(|l| l.chars().count())
        .collect();
    let bot_widths: Vec<usize> = plain_rich
        .lines()
        .filter(|l| !l.is_empty() && l.chars().all(|c| c == '─'))
        .map(|l| l.chars().count())
        .collect();
    assert_eq!(top_widths.len(), 3);
    assert_eq!(top_widths, bot_widths);
    assert!(
        top_widths.windows(2).all(|w| w[0] == w[1]),
        "all cards in thread should share width: {top_widths:?}"
    );
    assert!(
        plain_rich.contains("─\n Attachment: only.diff"),
        "attachment-only comment should not insert a blank body row:\n{plain_rich}"
    );
    assert!(
        plain_rich.contains("\n none "),
        "empty comment without attachments should show missing marker:\n{plain_rich}"
    );

    // ColorChoice::Always overrides NO_COLOR=1 for Block::Comments via anstyle.
    let env_no_color = output::Environment {
        no_color: Some("1".into()),
        term: Some("xterm-256color".into()),
        ..Default::default()
    };
    let always_out = Output::resolve(output::Settings {
        format: None,
        color: ColorChoice::Always,
        config_format: None,
        accessible: false,
        display: Display::from_width(100),
        json_output: true,
        tsv_output: true,
        fields: None,
        env: &env_no_color,
        stdout_terminal: false,
        stderr_terminal: false,
    })
    .unwrap();
    let forced = always_out.render(&report);
    assert!(forced.contains(&format!("{bold}#0{reset}")), "{forced:?}");
    assert!(
        forced.contains(&format!("{name}alice ┆ team{reset}")),
        "{forced:?}"
    );

    // Narrow 1-column fallback and non-Unicode styles color headers and attachments safely.
    let narrow_colored = Output::fixed(OutputMode::Rich, true, 25).render(&report);
    assert!(
        narrow_colored.contains(&format!("{bold}#0")),
        "{narrow_colored:?}"
    );
    assert!(
        narrow_colored.contains(&format!("{bold}Attachment:{reset}")),
        "{narrow_colored:?}"
    );
    let ascii_3col = output::RichRenderer {
        style: output::Style {
            color: true,
            unicode: false,
            width: 80,
        },
        time_zone: jiff::tz::TimeZone::UTC,
        relative_time: output::RelativeTime::Disabled,
    }
    .render(&report);
    assert!(ascii_3col.contains("+---"), "{ascii_3col}");
    assert!(
        ascii_3col.contains(&format!("{bold}#0{reset}")),
        "{ascii_3col:?}"
    );
    assert_eq!(
        ascii_3col
            .matches(&format!("{bold}Attachment:{reset}"))
            .count(),
        3,
        "{ascii_3col:?}"
    );
    let ascii_1col = output::RichRenderer {
        style: output::Style {
            color: true,
            unicode: false,
            width: 25,
        },
        time_zone: jiff::tz::TimeZone::UTC,
        relative_time: output::RelativeTime::Disabled,
    }
    .render(&report);
    assert!(ascii_1col.contains(" Attachment:"), "{ascii_1col}");
    assert!(
        ascii_1col.contains(&format!("{bold}Attachment:{reset}")),
        "{ascii_1col:?}"
    );
}

#[test]
fn git_comments_question_messages_and_local_comment_timestamps() {
    // Verify QuestionCommand::Messages uses Block::Comments in rich cards and plain records.
    let messages_raw = collection(vec![json!({
        "index": 1,
        "owner_link": "https://api.launchpad.net/devel/~helper",
        "date_created": "2026-11-17T10:30:00+00:00",
        "action": "Answer",
        "content": "Try reinstalling the package.",
    })]);
    let messages_report = question_messages_report(&Raw {
        value: serde_json::from_value(messages_raw["entries"].clone()).unwrap(),
        raw: messages_raw,
    });
    let fixed_tz = jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(2));
    let fixed_now: jiff::Timestamp = "2026-11-19T12:00:00+00:00".parse().unwrap();
    let rich_local = Output::fixed(OutputMode::Rich, false, 100)
        .with_time_zone(fixed_tz)
        .with_relative_time(output::RelativeTime::Fixed(fixed_now))
        .render(&messages_report);
    assert!(rich_local.contains("1 message found.\n\n"), "{rich_local}");
    assert!(
        rich_local.contains("#1 ┆ helper") && rich_local.contains("2026-11-17 12:30 (2 days ago)"),
        "{rich_local}"
    );
    assert!(rich_local.contains("Action: Answer"), "{rich_local}");
    assert!(
        rich_local.contains("Try reinstalling the package."),
        "{rich_local}"
    );

    let plain_utc = Output::fixed(OutputMode::Plain, false, 100).render(&messages_report);
    assert!(plain_utc.contains("1 message found.\n\n"), "{plain_utc}");
    assert!(plain_utc.contains("Message: #1\n"), "{plain_utc}");
    assert!(
        plain_utc.contains("Date: 2026-11-17 10:30\n"),
        "{plain_utc}"
    );
    assert!(!plain_utc.contains("days ago"), "{plain_utc}");
}
