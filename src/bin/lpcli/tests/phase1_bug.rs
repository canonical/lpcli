//! Phase 1 CLI tests for bug commands.
//!
//! Each test parses a real command line, runs it against a mock Launchpad
//! with [`bug_outcome`], and renders the result in the requested mode. JSON
//! snapshots replace the mock-server URL with `[server]` and redact
//! timestamps.

#[allow(unused_imports)]
use super::*;

use lpcli::error::{ErrorKind, LpError};

/// Launchpad target links stay on the production API host, because
/// `parse_target_link` only strips that prefix; they are never fetched.
const LP: &str = "https://api.launchpad.net/devel";

const TIMESTAMP_KEYS: [&str; 6] = [
    "date_created",
    "date_last_updated",
    "date_deleted",
    "date_last_checked",
    "date_last_changed",
    "date_last_message",
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn bug_command(argv: &[&str]) -> BugCommand {
    let cli = Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    validate_cli(&cli).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    let Command::Bug(command) = cli.command else {
        panic!("{argv:?} is not a bug command");
    };
    command
}

async fn run(server: &mockito::Server, argv: &[&str]) -> lpcli::error::Result<BugOutcome> {
    let client = mock_read_client(server, true);
    bug_outcome(&client, bug_command(argv), &Display::from_width(100)).await
}

fn render(outcome: &BugOutcome, mode: OutputMode, quiet: bool) -> String {
    outcome.render(&Output::fixed(mode, false, 100), quiet)
}

/// The JSON output, with the mock-server URL replaced by `[server]`.
fn json_output(server: &mockito::Server, outcome: &BugOutcome) -> serde_json::Value {
    let rendered = render(outcome, OutputMode::Json, false);
    serde_json::from_str(&rendered.replace(&server.url(), "[server]"))
        .expect("json output must always parse")
}

fn assert_json(name: &str, value: &serde_json::Value) {
    let mut settings = insta::Settings::clone_current();
    for key in TIMESTAMP_KEYS {
        settings.add_dynamic_redaction(&format!(".**.{key}"), |value, _| {
            if value.as_str().is_some() {
                insta::internals::Content::from("[timestamp]")
            } else {
                value
            }
        });
    }
    settings.bind(|| insta::assert_json_snapshot!(name, value));
}

fn assert_tsv(server: &mockito::Server, name: &str, outcome: &BugOutcome, records: usize) {
    let rendered = render(outcome, OutputMode::Tsv, false);
    crate::output::tests::assert_tsv_invariants(&rendered, records);
    let redacted: String = rendered
        .replace(&server.url(), "[server]")
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line
                .split('\t')
                .map(|field| match field.parse::<jiff::Timestamp>() {
                    Ok(_) => "[timestamp]",
                    Err(_) => field,
                })
                .collect();
            fields.join("\t") + "\n"
        })
        .collect();
    insta::assert_snapshot!(name, redacted);
}

fn collection(entries: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

async fn get(server: &mut mockito::Server, path: &str, body: &serde_json::Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

// ---------------------------------------------------------------------------
// Fixtures: Launchpad entries, including attributes lpcli does not model
// ---------------------------------------------------------------------------

fn bug_json(base: &str, tags: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "self_link": format!("{base}/bugs/42"),
        "web_link": "https://bugs.launchpad.net/bugs/42",
        "resource_type_link": format!("{base}/#bug"),
        "http_etag": "\"bug-etag\"",
        "id": 42,
        "title": "curl crashes on \"quoted\" URLs",
        "description": "Steps:\n1. curl 'http://x'\n2. crash",
        "tags": tags,
        "information_type": "Public",
        "heat": 12,
        "users_affected_count": 3,
        "owner_link": format!("{base}/~reporter"),
        "duplicate_of_link": null,
        "date_created": "2026-01-02T03:04:05.678901+00:00",
        "date_last_updated": "2026-02-03T04:05:06.789012+00:00",
        "bug_tasks_collection_link": format!("{base}/bugs/42/bug_tasks"),
    })
}

/// A bug task on `curl` in `series` (`None` for the distribution task).
fn task_json(base: &str, series: Option<&str>, status: &str) -> serde_json::Value {
    let path = match series {
        Some(series) => format!("ubuntu/{series}/+source/curl"),
        None => "ubuntu/+source/curl".to_string(),
    };
    let display = match series {
        Some(series) => format!("curl (Ubuntu {series})"),
        None => "curl (Ubuntu)".to_string(),
    };
    serde_json::json!({
        "self_link": format!("{base}/{path}/+bug/42"),
        "web_link": format!("https://bugs.launchpad.net/{path}/+bug/42"),
        "resource_type_link": format!("{base}/#bug_task"),
        "http_etag": "\"task-etag\"",
        "bug_link": format!("{base}/bugs/42"),
        "title": "Bug #42 in curl: \"crash\"",
        "status": status,
        "importance": "Medium",
        "assignee_link": null,
        "bug_target_display_name": display,
        "bug_target_name": display,
        "target_link": format!("{LP}/{path}"),
        "bug_watch_link": null,
        "milestone_link": null,
        "is_complete": false,
        "date_created": "2026-01-02T03:04:05.678901+00:00",
        "date_fix_released": null,
    })
}

fn attachment_json(base: &str, id: u64, data: bool) -> serde_json::Value {
    serde_json::json!({
        "self_link": format!("{base}/bugs/42/+attachment/{id}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/42/+attachment/{id}"),
        "resource_type_link": format!("{base}/#bug_attachment"),
        "http_etag": "\"attachment-etag\"",
        "title": format!("trace-{id}.txt"),
        "type": if id == 1 { "Patch" } else { "Unspecified" },
        "bug_link": format!("{base}/bugs/42"),
        "data_link": if data { serde_json::json!(format!("{base}/bugs/42/+attachment/{id}/data")) } else { serde_json::Value::Null },
        "message_link": format!("{base}/bugs/42/comments/1"),
    })
}

fn message_json(base: &str, index: u64, content: &str, deleted: bool) -> serde_json::Value {
    serde_json::json!({
        "self_link": format!("{base}/bugs/42/comments/{index}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/42/comments/{index}"),
        "resource_type_link": format!("{base}/#message"),
        "http_etag": "\"message-etag\"",
        "subject": "Re: curl crashes",
        "content": content,
        "owner_link": format!("{base}/~commenter"),
        "parent_link": null,
        "bug_attachments_collection_link": format!("{base}/bugs/42/comments/{index}/bug_attachments"),
        "date_created": "2026-01-03T00:00:00+00:00",
        "date_deleted": if deleted { serde_json::json!("2026-01-04T00:00:00+00:00") } else { serde_json::Value::Null },
        "date_last_edited": null,
    })
}

fn subscription_json(base: &str, person: &str) -> serde_json::Value {
    serde_json::json!({
        "self_link": format!("{base}/bugs/42/+subscription/{person}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/42/+subscription/{person}"),
        "resource_type_link": format!("{base}/#bug_subscription"),
        "http_etag": "\"subscription-etag\"",
        "bug_link": format!("{base}/bugs/42"),
        "person_link": format!("{base}/~{person}"),
        "subscribed_by_link": format!("{base}/~reporter"),
        "date_created": "2026-01-05T00:00:00+00:00",
    })
}

fn watch_json(base: &str, id: u64, remote_bug: &str) -> serde_json::Value {
    serde_json::json!({
        "self_link": format!("{base}/bugs/42/+watch/{id}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/42/+watch/{id}"),
        "resource_type_link": format!("{base}/#bug_watch"),
        "http_etag": "\"watch-etag\"",
        "bug_link": format!("{base}/bugs/42"),
        "bug_tracker_link": format!("{base}/bugs/bugtrackers/debbugs"),
        "remote_bug": remote_bug,
        "remote_status": "open",
        "remote_importance": null,
        "title": format!("Debian Bug tracker #{remote_bug}"),
        "url": format!("https://bugs.debian.org/cgi-bin/bugreport.cgi?bug={remote_bug}"),
        "owner_link": format!("{base}/~reporter"),
        "last_error_type": null,
        "date_created": "2026-01-06T00:00:00+00:00",
        "date_last_checked": null,
        "date_last_changed": null,
    })
}

// ---------------------------------------------------------------------------
// Output support
// ---------------------------------------------------------------------------

const READS: [&str; 8] = [
    "show -b 42",
    "tasks -b 42",
    "search --target ubuntu",
    "comments -b 42",
    "subscriptions -b 42",
    "attachments -b 42",
    "watches -b 42",
    "remote-bug-trackers",
];

const MUTATIONS: [&str; 15] = [
    "add-comment -b 42 -m hi",
    "delete-comment -b 42 -c 1",
    "create -T title -d description",
    "set-description -b 42 -d text",
    "set-status -b 42 --all-targets --all-series -s confirmed",
    "set-importance -b 42 -t curl --no-series -i high",
    "set-assignee -b 42 --many-targets curl --series noble -n someone",
    "subscribe -b 42 -n someone",
    "unsubscribe -b 42 -n someone",
    "add-task -b 42 -p curl --many-series noble,jammy",
    "delete-task -b 42 -p curl",
    "tag -b 42 --add-tag foo",
    "link-remote -b 42 --github canonical/apport#658",
    "unlink-remote -b 42",
    "tracker create --base-url https://github.com/canonical/apport/issues",
];

fn argv(format: &str, args: &str) -> Vec<String> {
    let mut argv: Vec<String> = ["lpcli", "--format", format, "bug"]
        .into_iter()
        .map(str::to_string)
        .collect();
    argv.extend(shlex::split(args).unwrap());
    argv
}

#[test]
fn every_bug_read_and_mutation_accepts_json() {
    for args in READS.iter().chain(MUTATIONS.iter()) {
        let cli = Cli::try_parse_from(argv("json", args)).unwrap();
        validate_cli(&cli).unwrap_or_else(|e| panic!("{args}: {e}"));
    }
}

#[test]
fn tsv_is_only_available_for_list_reads() {
    for args in READS.iter().filter(|args| !args.starts_with("show")) {
        let cli = Cli::try_parse_from(argv("tsv", args)).unwrap();
        validate_cli(&cli).unwrap_or_else(|e| panic!("{args}: {e}"));
    }
    for args in std::iter::once(&"show -b 42").chain(MUTATIONS.iter()) {
        let cli = Cli::try_parse_from(argv("tsv", args)).unwrap();
        let error = validate_cli(&cli).expect_err(args);
        assert_eq!(error.exit_code(), 2, "{args}");
        assert!(error.to_string().contains("--format tsv"), "{args}");
    }
}

#[test]
fn quiet_is_accepted_for_bug_mutations_only() {
    for args in MUTATIONS {
        let mut full = argv("json", args);
        full.push("--quiet".into());
        validate_cli(&Cli::try_parse_from(&full).unwrap())
            .unwrap_or_else(|e| panic!("{args}: {e}"));
    }
    for args in READS {
        let mut full = argv("json", args);
        full.push("--quiet".into());
        let error = validate_cli(&Cli::try_parse_from(&full).unwrap()).expect_err(args);
        assert_eq!(error.exit_code(), 2, "{args}");
    }
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_json_combines_unmodified_launchpad_members() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let bug = bug_json(&base, &["crash", "noble"]);
    let tasks = collection(vec![
        task_json(&base, None, "Confirmed"),
        task_json(&base, Some("noble"), "In Progress"),
    ]);
    let attachments = collection(vec![
        attachment_json(&base, 1, true),
        attachment_json(&base, 2, false),
    ]);
    let mocks = [
        get(&mut server, "/bugs/42", &bug).await,
        get(&mut server, "/bugs/42/bug_tasks", &tasks).await,
        get(&mut server, "/bugs/42/attachments", &attachments).await,
        server
            .mock("GET", "/bugs/42/+attachment/1/data")
            .with_status(303)
            .with_header("location", "https://launchpadlibrarian.net/1/trace-1.txt")
            .create_async()
            .await,
    ];

    let outcome = run(&server, &["lpcli", "bug", "show", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered["bug"], bug, "the bug entry must be unmodified");
    assert_eq!(rendered["bug_tasks"], tasks);
    assert_eq!(rendered["attachments"], attachments);
    assert_eq!(
        rendered["lpcli"]["attachment_download_urls"][0]["download_url"],
        "https://launchpadlibrarian.net/1/trace-1.txt"
    );
    assert!(rendered["lpcli"]["attachment_download_urls"][1]["download_url"].is_null());
    assert_json("bug_show_json", &json_output(&server, &outcome));

    let rich = render(&outcome, OutputMode::Rich, false);
    assert!(
        rich.contains("Bug #42") && rich.contains("trace-1.txt"),
        "{rich}"
    );
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn show_fields_select_launchpad_attributes_in_each_member() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(&mut server, "/bugs/42", &bug_json(&base, &[])).await;
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![task_json(&base, None, "New")]),
    )
    .await;
    get(&mut server, "/bugs/42/attachments", &collection(vec![])).await;

    let outcome = run(&server, &["lpcli", "bug", "show", "-b", "42"])
        .await
        .unwrap();
    let output = Output::fixed(OutputMode::Json, false, 100).with_fields(&["title", "status"]);
    let value: serde_json::Value = serde_json::from_str(&outcome.render(&output, false)).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "bug": {"title": "curl crashes on \"quoted\" URLs"},
            "bug_tasks": {"total_size": 1, "start": 0, "entries": [
                {"title": "Bug #42 in curl: \"crash\"", "status": "New"},
            ]},
            "attachments": {"total_size": 0, "start": 0, "entries": []},
            "lpcli": {"attachment_download_urls": []},
        })
    );
}

#[tokio::test]
async fn tasks_json_is_the_launchpad_collection_and_tsv_lists_tasks() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let next = format!("{base}/bugs/42/bug_tasks?ws.start=1&ws.size=1");
    let first = task_json(&base, None, "Confirmed");
    let second = task_json(&base, Some("noble"), "Fix Released");
    let page_one = get(
        &mut server,
        "/bugs/42/bug_tasks",
        &serde_json::json!({"total_size": 2, "start": 0, "entries": [first], "next_collection_link": next}),
    )
    .await;
    let page_two = server
        .mock("GET", "/bugs/42/bug_tasks")
        .match_query(mockito::Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_body(
            serde_json::json!({"total_size": 2, "start": 1, "entries": [second]}).to_string(),
        )
        .create_async()
        .await;

    let outcome = run(&server, &["lpcli", "bug", "tasks", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(
        rendered,
        serde_json::json!({"total_size": 2, "start": 0, "entries": [first, second]}),
        "every page is fetched, so next_collection_link is omitted"
    );
    assert_json("bug_tasks_json", &json_output(&server, &outcome));
    assert_tsv(&server, "bug_tasks_tsv", &outcome, 2);
    page_one.assert_async().await;
    page_two.assert_async().await;
}

#[tokio::test]
async fn comments_json_combines_messages_and_attachments() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let messages = collection(vec![
        message_json(&base, 0, "Original\ndescription", false),
        message_json(&base, 1, "See attached\ttrace.", false),
        message_json(&base, 2, "", true),
    ]);
    let attachments = collection(vec![attachment_json(&base, 1, false)]);
    get(&mut server, "/bugs/42/messages", &messages).await;
    get(&mut server, "/bugs/42/attachments", &attachments).await;

    let outcome = run(&server, &["lpcli", "bug", "comments", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered["messages"], messages);
    assert_eq!(rendered["attachments"], attachments);
    assert_json("bug_comments_json", &json_output(&server, &outcome));
    assert_tsv(&server, "bug_comments_tsv", &outcome, 3);
    let rich = render(&outcome, OutputMode::Rich, false);
    assert!(
        rich.contains("trace-1.txt") && !rich.contains("Comment #2"),
        "{rich}"
    );
}

#[tokio::test]
async fn attachments_json_keeps_resolved_urls_under_lpcli() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let attachments = collection(vec![
        attachment_json(&base, 1, true),
        attachment_json(&base, 2, false),
    ]);
    get(&mut server, "/bugs/42/attachments", &attachments).await;
    let redirect = server
        .mock("GET", "/bugs/42/+attachment/1/data")
        .with_status(303)
        .with_header("location", "https://launchpadlibrarian.net/1/trace-1.txt")
        .create_async()
        .await;

    let outcome = run(&server, &["lpcli", "bug", "attachments", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered["attachments"], attachments);
    assert!(
        rendered["attachments"]["entries"][0]
            .get("download_url")
            .is_none(),
        "lpcli values are never added to Launchpad entries"
    );
    assert_json("bug_attachments_json", &json_output(&server, &outcome));
    assert_tsv(&server, "bug_attachments_tsv", &outcome, 2);
    redirect.assert_async().await;
}

#[tokio::test]
async fn attachments_empty_in_every_mode() {
    let mut server = isolated_mock_server().await;
    get(&mut server, "/bugs/42/attachments", &collection(vec![])).await;
    let outcome = run(&server, &["lpcli", "bug", "attachments", "-b", "42"])
        .await
        .unwrap();
    assert_eq!(
        render(&outcome, OutputMode::Rich, false),
        "No attachments for bug #42.\n"
    );
    assert_eq!(
        render(&outcome, OutputMode::Tsv, false),
        "title\ttype\tdownload_url\tself_link\n"
    );
    let value: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(value["attachments"]["entries"], serde_json::json!([]));
}

#[tokio::test]
async fn subscriptions_json_is_the_launchpad_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let subs = collection(vec![
        subscription_json(&base, "reporter"),
        subscription_json(&base, "ubuntu-bugs"),
    ]);
    get(&mut server, "/bugs/42/subscriptions", &subs).await;

    let outcome = run(&server, &["lpcli", "bug", "subscriptions", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered, subs);
    assert_json("bug_subscriptions_json", &json_output(&server, &outcome));
    assert_tsv(&server, "bug_subscriptions_tsv", &outcome, 2);
}

#[tokio::test]
async fn watches_json_reports_linked_tasks_under_lpcli() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let watches = collection(vec![
        watch_json(&base, 7, "1150409"),
        watch_json(&base, 8, "1150410"),
    ]);
    let mut linked = task_json(&base, None, "Confirmed");
    // A beta-namespace link still matches the watch by ID.
    linked["bug_watch_link"] = "https://api.launchpad.net/beta/bugs/42/+watch/7".into();
    let tasks = collection(vec![linked, task_json(&base, Some("noble"), "New")]);
    get(&mut server, "/bugs/42/bug_watches", &watches).await;
    get(&mut server, "/bugs/42/bug_tasks", &tasks).await;

    let outcome = run(&server, &["lpcli", "bug", "watches", "-b", "42"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered["bug_watches"], watches);
    assert_eq!(rendered["bug_tasks"], tasks);
    assert_eq!(
        rendered["lpcli"]["linked_bug_tasks"][1]["bug_task_links"],
        serde_json::json!([]),
        "an orphaned watch has no linked tasks"
    );
    assert_json("bug_watches_json", &json_output(&server, &outcome));
    assert_tsv(&server, "bug_watches_tsv", &outcome, 2);
    let rich = render(&outcome, OutputMode::Rich, false);
    assert!(
        rich.contains("orphan") && rich.contains("curl (Ubuntu)"),
        "{rich}"
    );
}

#[tokio::test]
async fn watches_task_failure_propagates_instead_of_reporting_orphans() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_watches",
        &collection(vec![watch_json(&base, 7, "1")]),
    )
    .await;
    server
        .mock("GET", "/bugs/42/bug_tasks")
        .with_status(403)
        .create_async()
        .await;
    let error = run(&server, &["lpcli", "bug", "watches", "-b", "42"])
        .await
        .expect_err("a failed task read must not render orphans");
    assert_eq!(error.kind(), ErrorKind::PermissionDenied);
}

#[tokio::test]
async fn search_json_is_the_launchpad_page_through_dispatch() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let page = serde_json::json!({
        "total_size": 5,
        "start": 0,
        "entries": [task_json(&base, None, "New")],
        "next_collection_link": format!("{base}/ubuntu?ws.op=searchTasks&ws.start=1"),
    });
    server
        .mock("GET", "/ubuntu")
        .match_query(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "searchTasks".into(),
        ))
        .with_body(page.to_string())
        .create_async()
        .await;
    let outcome = run(&server, &["lpcli", "bug", "search", "--limit", "1"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered, page, "--limit keeps next_collection_link");
    assert_json("bug_search_dispatch_json", &json_output(&server, &outcome));
}

#[tokio::test]
async fn remote_bug_trackers_json_through_dispatch() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let tracker = serde_json::json!({
        "self_link": format!("{base}/bugs/bugtrackers/debbugs"),
        "resource_type_link": format!("{base}/#bug_tracker"),
        "name": "debbugs",
        "title": "Debian Bug tracker",
        "bug_tracker_type": "Debbugs",
        "base_url": "https://bugs.debian.org/",
        "active": true,
    });
    server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::Any)
        .with_body(collection(vec![tracker.clone()]).to_string())
        .create_async()
        .await;
    let outcome = run(&server, &["lpcli", "bug", "remote-bug-trackers"])
        .await
        .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered, collection(vec![tracker]));
    assert_json(
        "bug_remote_bug_trackers_dispatch_json",
        &json_output(&server, &outcome),
    );
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Mock a PATCH of `task` that returns the task with `status`.
async fn patch_task(
    server: &mut mockito::Server,
    base: &str,
    series: Option<&str>,
    status: &str,
) -> mockito::Mock {
    let task = task_json(base, series, status);
    let path = task["self_link"].as_str().unwrap().replace(base, "");
    server
        .mock("PATCH", path.as_str())
        .match_body(mockito::Matcher::Json(
            serde_json::json!({"status": status}),
        ))
        .with_body(task.to_string())
        .create_async()
        .await
}

#[tokio::test]
async fn set_status_prints_updated_tasks_and_quiet_self_links() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "New"),
            task_json(&base, Some("noble"), "New"),
        ]),
    )
    .await;
    let patches = [
        patch_task(&mut server, &base, None, "Confirmed").await,
        patch_task(&mut server, &base, Some("noble"), "Confirmed").await,
    ];
    let argv = [
        "lpcli",
        "bug",
        "set-status",
        "-b",
        "42",
        "--all-targets",
        "--all-series",
        "-s",
        "confirmed",
    ];
    let outcome = run(&server, &argv).await.unwrap();
    assert!(outcome.error.is_none());
    assert_json("bug_set_status_json", &json_output(&server, &outcome));
    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        format!("{base}/ubuntu/+source/curl/+bug/42\n{base}/ubuntu/noble/+source/curl/+bug/42\n")
    );
    assert_eq!(
        render(&outcome, OutputMode::Rich, false),
        "✓ Status updated to 'Confirmed' for 2 task(s) on bug #42.\n"
    );
    for patch in patches {
        patch.assert_async().await;
    }
}

#[tokio::test]
async fn set_status_partial_failure_prints_both_and_exits_partial() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "New"),
            task_json(&base, Some("noble"), "New"),
        ]),
    )
    .await;
    patch_task(&mut server, &base, None, "Triaged").await;
    server
        .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
        .with_status(403)
        .with_body("not allowed oauth_token=abc123")
        .create_async()
        .await;
    let argv = [
        "lpcli",
        "bug",
        "set-status",
        "-b",
        "42",
        "--all-targets",
        "--all-series",
        "-s",
        "triaged",
    ];
    let outcome = run(&server, &argv).await.unwrap();

    let error = outcome
        .error
        .as_ref()
        .expect("a partial failure is an error");
    assert_eq!(error.kind(), ErrorKind::Partial);
    assert_eq!(error.exit_code(), 7);
    let message = error.to_string();
    assert!(message.contains("Updated 1 of 2"), "{message}");
    assert!(message.contains("curl (Ubuntu noble)"), "{message}");

    let json = json_output(&server, &outcome);
    assert_eq!(json["bug_tasks"]["total_size"], 1);
    assert_eq!(
        json["lpcli"]["failures"][0]["error"]["kind"],
        "permission_denied"
    );
    let failure_text = json["lpcli"]["failures"][0]["error"]["message"]
        .as_str()
        .unwrap();
    assert!(
        !failure_text.contains("abc123"),
        "secrets must be redacted: {failure_text}"
    );
    assert_json("bug_set_status_partial_json", &json);

    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        format!("{base}/ubuntu/+source/curl/+bug/42\n"),
        "--quiet prints only what succeeded"
    );
    let output = Output::fixed(OutputMode::Plain, false, 100);
    assert!(
        outcome
            .render_warnings(&output)
            .contains("Failed: set_status")
    );
    let json_output = Output::fixed(OutputMode::Json, false, 100);
    assert_eq!(
        outcome.render_warnings(&json_output),
        "",
        "JSON reports failures inline"
    );
}

#[tokio::test]
async fn set_importance_total_failure_keeps_the_original_error() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![task_json(&base, Some("noble"), "New")]),
    )
    .await;
    server
        .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
        .with_status(401)
        .create_async()
        .await;
    let argv = [
        "lpcli",
        "bug",
        "set-importance",
        "-b",
        "42",
        "-t",
        "curl",
        "--series",
        "noble",
        "-i",
        "high",
    ];
    let outcome = run(&server, &argv).await.unwrap();
    let error = outcome.error.as_ref().unwrap();
    assert_eq!(
        error.kind(),
        ErrorKind::NotAuthenticated,
        "nothing was changed"
    );
    assert_eq!(render(&outcome, OutputMode::Json, false), "");
    assert_eq!(render(&outcome, OutputMode::Json, true), "");
    let output = Output::fixed(OutputMode::Rich, false, 100);
    assert_eq!(
        outcome.render_warnings(&output),
        "",
        "the error itself reports the failure"
    );
}

#[tokio::test]
async fn add_comment_prints_the_created_message_and_quiet_self_link() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let message = message_json(&base, 3, "Thanks!", false);
    let post = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "newMessage".into()),
            mockito::Matcher::UrlEncoded("content".into(), "Thanks!".into()),
        ]))
        .with_status(201)
        .with_header("location", &format!("{base}/bugs/42/comments/3"))
        .create_async()
        .await;
    let read_back = get(&mut server, "/bugs/42/comments/3", &message).await;

    let outcome = run(
        &server,
        &["lpcli", "bug", "add-comment", "-b", "42", "-m", "Thanks!"],
    )
    .await
    .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered, message, "the read-back entry, unmodified");
    assert_json("bug_add_comment_json", &json_output(&server, &outcome));
    assert_eq!(
        render(&outcome, OutputMode::Rich, true),
        format!("{base}/bugs/42/comments/3\n")
    );
    post.assert_async().await;
    read_back.assert_async().await;
}

#[tokio::test]
async fn add_comment_without_location_is_partial() {
    let mut server = isolated_mock_server().await;
    server
        .mock("POST", "/bugs/42")
        .with_status(201)
        .create_async()
        .await;
    let error = run(
        &server,
        &["lpcli", "bug", "add-comment", "-b", "42", "-m", "hi"],
    )
    .await
    .expect_err("an unconfirmed comment must not look like success");
    assert_eq!(error.exit_code(), 7, "{error}");
}

#[tokio::test]
async fn tag_prints_the_updated_bug_entry() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(&mut server, "/bugs/42", &bug_json(&base, &["crash"])).await;
    let updated = bug_json(&base, &["crash", "regression"]);
    let patch = server
        .mock("PATCH", "/bugs/42")
        .match_body(mockito::Matcher::Json(
            serde_json::json!({"tags": ["crash", "regression"]}),
        ))
        .with_body(updated.to_string())
        .create_async()
        .await;
    let outcome = run(
        &server,
        &["lpcli", "bug", "tag", "-b", "42", "--add-tag", "regression"],
    )
    .await
    .unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(rendered, updated);
    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        format!("{base}/bugs/42\n")
    );
    patch.assert_async().await;
}

#[tokio::test]
async fn unsubscribe_has_no_entry_so_quiet_prints_nothing() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "unsubscribe".into(),
        ))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let outcome = run(
        &server,
        &["lpcli", "bug", "unsubscribe", "-b", "42", "-n", "someone"],
    )
    .await
    .unwrap();
    assert_eq!(render(&outcome, OutputMode::Json, true), "");
    let value: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"lpcli": {
            "bug_id": 42,
            "person_link": format!("{base}/~someone"),
            "unsubscribed": true,
        }})
    );
}

#[tokio::test]
async fn add_task_partial_series_failure_reports_created_tasks() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let noble = task_json(&base, Some("noble"), "New");
    server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::UrlEncoded(
            "target".into(),
            format!("{base}/ubuntu/noble/+source/curl"),
        ))
        .with_status(201)
        .with_header(
            "location",
            &format!("{base}/ubuntu/noble/+source/curl/+bug/42"),
        )
        .create_async()
        .await;
    server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::UrlEncoded(
            "target".into(),
            format!("{base}/ubuntu/jammy/+source/curl"),
        ))
        .with_status(400)
        .with_body("A fix for this bug has already been requested for Jammy")
        .create_async()
        .await;
    get(&mut server, "/ubuntu/noble/+source/curl/+bug/42", &noble).await;

    let argv = [
        "lpcli",
        "bug",
        "add-task",
        "-b",
        "42",
        "-p",
        "curl",
        "--many-series",
        "noble,jammy",
    ];
    let outcome = run(&server, &argv).await.unwrap();
    let error = outcome.error.as_ref().expect("one series failed");
    assert_eq!(error.exit_code(), 7);
    assert!(error.to_string().contains("Created 1 of 2"), "{error}");

    let json = json_output(&server, &outcome);
    assert_eq!(
        json["bug_tasks"]["entries"][0]["bug_target_display_name"],
        "curl (Ubuntu noble)"
    );
    assert_eq!(json["lpcli"]["failures"][0]["target"], "jammy");
    assert_json("bug_add_task_partial_json", &json);
    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        format!("{base}/ubuntu/noble/+source/curl/+bug/42\n")
    );
}

#[tokio::test]
async fn delete_task_partial_failure_lists_deleted_links() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "New"),
            task_json(&base, Some("noble"), "New"),
        ]),
    )
    .await;
    server
        .mock("DELETE", "/ubuntu/+source/curl/+bug/42")
        .with_status(200)
        .create_async()
        .await;
    server
        .mock("DELETE", "/ubuntu/noble/+source/curl/+bug/42")
        .with_status(403)
        .create_async()
        .await;
    let outcome = run(
        &server,
        &["lpcli", "bug", "delete-task", "-b", "42", "-p", "curl"],
    )
    .await
    .unwrap();
    let error = outcome.error.as_ref().unwrap();
    assert_eq!(error.exit_code(), 7);
    assert!(error.to_string().contains("Deleted 1 of 2"), "{error}");
    let json = json_output(&server, &outcome);
    assert_eq!(
        json["lpcli"]["deleted_task_links"],
        serde_json::json!(["[server]/ubuntu/+source/curl/+bug/42"])
    );
    assert_json("bug_delete_task_partial_json", &json);
    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        "",
        "no entry remains"
    );
}

#[tokio::test]
async fn create_with_failed_attachment_is_partial_and_keeps_the_bug() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let bug = bug_json(&base, &[]);
    server
        .mock("POST", "/bugs")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "createBug".into(),
        ))
        .with_status(201)
        .with_header("location", &format!("{base}/bugs/42"))
        .create_async()
        .await;
    get(&mut server, "/bugs/42", &bug).await;
    server
        .mock("POST", "/bugs/42")
        .with_status(500)
        .with_body("librarian unavailable")
        .create_async()
        .await;
    let argv = [
        "lpcli",
        "bug",
        "create",
        "-T",
        "title",
        "-d",
        "description",
        "-u",
        "https://example.org/log",
    ];
    let outcome = run(&server, &argv).await.unwrap();
    let error = outcome.error.as_ref().unwrap();
    assert_eq!(error.exit_code(), 7);
    assert!(error.to_string().contains("Bug #42 was created"), "{error}");
    let value: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(value["bug"], bug);
    assert!(value["attachment"].is_null());
    assert_eq!(value["lpcli"]["failures"][0]["operation"], "add_attachment");
    assert_eq!(
        render(&outcome, OutputMode::Json, true),
        format!("{base}/bugs/42\n")
    );
}

#[test]
fn total_failure_of_several_steps_lists_every_failure() {
    let failures = vec![
        BugFailure::new(
            "set_status",
            Some("a".into()),
            None,
            LpError::Api {
                status: 403,
                message: "no".into(),
            },
        ),
        BugFailure::new(
            "set_status",
            Some("b".into()),
            None,
            LpError::LocalNotFound("gone".into()),
        ),
    ];
    let json = serde_json::json!({"lpcli": {"failures": ["a", "b"]}});
    let outcome = finish_batch(
        BugOutcome::mutation(json.clone(), "✓ Updated 0 task(s).\n".into(), Vec::new()),
        0,
        failures,
        |_| unreachable!("nothing was applied"),
    );
    assert_eq!(
        outcome.error.as_ref().unwrap().kind(),
        ErrorKind::PermissionDenied
    );
    let value: serde_json::Value =
        serde_json::from_str(&render(&outcome, OutputMode::Json, false)).unwrap();
    assert_eq!(value, json);
    assert_eq!(
        render(&outcome, OutputMode::Rich, false),
        "",
        "no misleading success line"
    );
    assert_eq!(render(&outcome, OutputMode::Json, true), "");
    let warnings = outcome.render_warnings(&Output::fixed(OutputMode::Plain, false, 100));
    assert!(
        warnings.contains("'a'") && warnings.contains("'b'"),
        "{warnings}"
    );
}

#[test]
fn quiet_links_are_terminal_safe() {
    let outcome = BugOutcome::mutation(
        serde_json::json!({}),
        String::new(),
        vec!["https://api.launchpad.net/devel/bugs/1\u{1b}[2J".into()],
    );
    assert_eq!(
        render(&outcome, OutputMode::Rich, true),
        "https://api.launchpad.net/devel/bugs/1\u{FFFD}[2J\n"
    );
}
