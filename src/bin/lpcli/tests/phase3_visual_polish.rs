//! Phase 3 tests: Rich snapshots for bug commands, semantic ANSI colors,
//! normalized usernames/dates, and `$PAGER` / `--no-pager` support.

use serde_json::{Value, json};

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
                        Column {
                            label: "Status",
                            key: "status",
                        },
                        Column {
                            label: "Importance",
                            key: "importance",
                        },
                        Column {
                            label: "Assignee",
                            key: "assignee",
                        },
                        Column {
                            label: "Person",
                            key: "person",
                        },
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

    // Header row labels stay uncolored by value styles.
    assert!(
        colored.contains("│ Status    ┆ Importance ┆ Assignee ┆ Person     │"),
        "table header should stay uncolored:\n{colored:?}"
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
    for rendered in [render_rich(&bug_out, false), render_plain(&bug_out)] {
        assert!(rendered.contains("Author: jdoe"), "{rendered}");
        assert!(rendered.contains("Date: 2026-11-16 09:15"), "{rendered}");
        assert!(!rendered.contains(&base), "{rendered}");
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
    for mode in [OutputMode::Rich, OutputMode::Plain] {
        let rendered = Output::fixed(mode, false, 100).render(&report);
        assert!(rendered.contains("Author: alice"), "{rendered}");
        assert!(rendered.contains("Author: reviewer"), "{rendered}");
        assert!(!rendered.contains("~reviewer"), "{rendered}");
        assert!(rendered.contains("Date: 2026-11-16 14:22"), "{rendered}");
        assert!(rendered.contains("Date: 2026-11-16 09:15"), "{rendered}");
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
                    Column {
                        label: "Title",
                        key: "title",
                    },
                    Column {
                        label: "Status",
                        key: "status",
                    },
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
                columns: vec![
                    Column {
                        label: "Id",
                        key: "id",
                    },
                    Column {
                        label: "Status",
                        key: "status",
                    },
                ],
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
    let wrapped = Output::fixed(OutputMode::Rich, true, 20).render(&wrap_report);
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
                    Column {
                        label: "Summary",
                        key: "summary",
                    },
                    Column {
                        label: "Status",
                        key: "status",
                    },
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
