//! Phase 2 CLI accessibility (`plain`) tests.
//!
//! Verifies that all commands — especially every `bug` read, mutation, and
//! partial failure — produce screen-reader and braille-friendly `plain` output,
//! that `--progress` and `resolve_progress` behave as specified, that
//! `clean_plain` strips or translates banned decorative characters, and that
//! security-key selection prompts state choices clearly by number.

#[allow(unused_imports)]
use super::*;

use crate::output::tests::assert_plain_invariants;
use lpcli::{download, error::ErrorKind, hardware_key};
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";

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

fn render_plain(outcome: &BugOutcome) -> String {
    let rendered = outcome.render(&Output::fixed(OutputMode::Plain, false, 100), false);
    assert_plain_invariants(&rendered);
    rendered
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

async fn get(server: &mut mockito::Server, path: &str, body: &Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

fn bug_json(base: &str, tags: &[&str]) -> Value {
    json!({
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

fn task_json(base: &str, series: Option<&str>, status: &str, assignee: Option<&str>) -> Value {
    let path = match series {
        Some(series) => format!("ubuntu/{series}/+source/curl"),
        None => "ubuntu/+source/curl".to_string(),
    };
    let display = match series {
        Some(series) => format!("curl (Ubuntu {series})"),
        None => "curl (Ubuntu)".to_string(),
    };
    json!({
        "self_link": format!("{base}/{path}/+bug/42"),
        "web_link": format!("https://bugs.launchpad.net/{path}/+bug/42"),
        "resource_type_link": format!("{base}/#bug_task"),
        "http_etag": "\"task-etag\"",
        "bug_link": format!("{base}/bugs/42"),
        "title": "Bug #42 in curl: \"crash\"",
        "status": status,
        "importance": "Medium",
        "assignee_link": assignee.map(|name| format!("{base}/~{name}")),
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

fn attachment_json(base: &str, id: u64, data: bool) -> Value {
    json!({
        "self_link": format!("{base}/bugs/42/+attachment/{id}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/42/+attachment/{id}"),
        "resource_type_link": format!("{base}/#bug_attachment"),
        "http_etag": "\"attachment-etag\"",
        "title": format!("trace-{id}.txt"),
        "type": if id == 1 { "Patch" } else { "Unspecified" },
        "bug_link": format!("{base}/bugs/42"),
        "data_link": if data {
            json!(format!("{base}/bugs/42/+attachment/{id}/data"))
        } else {
            Value::Null
        },
        "message_link": format!("{base}/bugs/42/comments/1"),
    })
}

fn message_json(base: &str, index: u64, content: &str, deleted: bool) -> Value {
    json!({
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
        "date_deleted": if deleted { json!("2026-01-04T00:00:00+00:00") } else { Value::Null },
        "date_last_edited": null,
    })
}

fn subscription_json(base: &str, person: &str) -> Value {
    json!({
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

fn watch_json(base: &str, id: u64, remote_bug: &str) -> Value {
    json!({
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
// Plain output for bug read commands
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_show_plain_has_labeled_fields_explicit_series_and_counts() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42",
        &bug_json(&base, &["crash", "noble"]),
    )
    .await;
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "Confirmed", Some("alice")),
            task_json(&base, Some("noble"), "In Progress", None),
        ]),
    )
    .await;
    get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![
            attachment_json(&base, 1, true),
            attachment_json(&base, 2, false),
        ]),
    )
    .await;
    server
        .mock("GET", "/bugs/42/+attachment/1/data")
        .with_status(303)
        .with_header("location", "https://launchpadlibrarian.net/1/trace-1.txt")
        .create_async()
        .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "show", "-b", "42"])
        .await
        .unwrap();
    let plain = render_plain(&outcome);
    assert!(plain.contains("2 bug tasks found."), "{plain}");
    assert!(plain.contains("Series: noble\n"), "{plain}");
    assert!(
        !plain.contains("\n    curl"),
        "plain output must state series in words rather than indentation:\n{plain}"
    );
    assert!(plain.contains("2 attachments found."), "{plain}");
    insta::assert_snapshot!("bug_show_plain", plain.replace(&base, "[server]"));
}

#[tokio::test]
async fn bug_tasks_plain_lists_count_first_and_series_field() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "Confirmed", None),
            task_json(&base, Some("noble"), "Fix Released", Some("bob")),
        ]),
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "tasks", "-b", "42"])
        .await
        .unwrap();
    let plain = render_plain(&outcome);
    assert!(plain.starts_with("2 bug tasks found.\n\n"), "{plain}");
    assert!(plain.contains("Series: noble\n"), "{plain}");
    assert!(plain.contains("Assignee: none\n"), "{plain}");
    assert!(plain.contains("Assignee: bob\n"), "{plain}");
    insta::assert_snapshot!("bug_tasks_plain", plain);

    // Empty tasks list uses a clear sentence.
    let mut empty_server = isolated_mock_server().await;
    get(&mut empty_server, "/bugs/99/bug_tasks", &collection(vec![])).await;
    let empty_outcome = run_bug(&empty_server, &["lpcli", "bug", "tasks", "-b", "99"])
        .await
        .unwrap();
    assert_eq!(
        render_plain(&empty_outcome),
        "No bug tasks found for bug #99.\n"
    );
}

#[tokio::test]
async fn bug_comments_plain_uses_attachment_word_and_skips_deleted() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/messages",
        &collection(vec![
            message_json(&base, 0, "Original\ndescription", false),
            message_json(&base, 1, "See attached\ttrace — thanks.", false),
            message_json(&base, 2, "Deleted text", true),
        ]),
    )
    .await;
    get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![attachment_json(&base, 1, false)]),
    )
    .await;

    let outcome = run_bug(&server, &["lpcli", "bug", "comments", "-b", "42"])
        .await
        .unwrap();
    let plain = render_plain(&outcome);
    assert!(plain.starts_with("2 comments found.\n\n"), "{plain}");
    assert!(plain.contains("Attachment: trace-1.txt [Patch]"), "{plain}");
    assert!(!plain.contains("Comment: #2"), "{plain}");
    assert!(!plain.contains('📎'), "{plain}");
    assert!(!plain.contains('—'), "{plain}");
    insta::assert_snapshot!("bug_comments_plain", plain);
}

#[tokio::test]
async fn bug_subscriptions_attachments_and_watches_plain_snapshots() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/subscriptions",
        &collection(vec![
            subscription_json(&base, "reporter"),
            subscription_json(&base, "ubuntu-bugs"),
        ]),
    )
    .await;
    get(
        &mut server,
        "/bugs/42/attachments",
        &collection(vec![
            attachment_json(&base, 1, true),
            attachment_json(&base, 2, false),
        ]),
    )
    .await;
    server
        .mock("GET", "/bugs/42/+attachment/1/data")
        .with_status(303)
        .with_header("location", "https://launchpadlibrarian.net/1/trace-1.txt")
        .create_async()
        .await;
    let mut linked = task_json(&base, None, "Confirmed", None);
    linked["bug_watch_link"] = format!("{base}/bugs/42/+watch/7").into();
    get(
        &mut server,
        "/bugs/42/bug_watches",
        &collection(vec![
            watch_json(&base, 7, "1150409"),
            watch_json(&base, 8, "1150410"),
        ]),
    )
    .await;
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![linked, task_json(&base, Some("noble"), "New", None)]),
    )
    .await;

    let subs = render_plain(
        &run_bug(&server, &["lpcli", "bug", "subscriptions", "-b", "42"])
            .await
            .unwrap(),
    );
    assert!(subs.starts_with("2 subscriptions found.\n\n"), "{subs}");
    insta::assert_snapshot!("bug_subscriptions_plain", subs);

    let attachments = render_plain(
        &run_bug(&server, &["lpcli", "bug", "attachments", "-b", "42"])
            .await
            .unwrap(),
    );
    assert!(
        attachments.starts_with("2 attachments found.\n\n"),
        "{attachments}"
    );
    insta::assert_snapshot!("bug_attachments_plain", attachments);

    let watches = render_plain(
        &run_bug(&server, &["lpcli", "bug", "watches", "-b", "42"])
            .await
            .unwrap(),
    );
    assert!(watches.starts_with("2 bug watches found.\n\n"), "{watches}");
    insta::assert_snapshot!("bug_watches_plain", watches);
}

// ---------------------------------------------------------------------------
// Plain output for every bug mutation command and partial failure warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_mutations_use_word_prefixes_and_pass_plain_invariants() {
    let mut server = isolated_mock_server().await;
    let base = server.url();

    // 1. add-comment
    let comment = message_json(&base, 3, "Thanks!", false);
    server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "newMessage".into()))
        .with_status(201)
        .with_header("location", &format!("{base}/bugs/42/comments/3"))
        .create_async()
        .await;
    get(&mut server, "/bugs/42/comments/3", &comment).await;
    let plain = render_plain(
        &run_bug(
            &server,
            &["lpcli", "bug", "add-comment", "-b", "42", "-m", "Thanks!"],
        )
        .await
        .unwrap(),
    );
    assert_eq!(plain, "OK: Comment added to bug #42.\n");

    // 2. delete-comment
    get(
        &mut server,
        "/bugs/42/messages",
        &collection(vec![
            message_json(&base, 0, "desc", false),
            message_json(&base, 1, "Thanks!", false),
        ]),
    )
    .await;
    get(&mut server, "/bugs/42/attachments", &collection(vec![])).await;
    server
        .mock("POST", "/bugs/42/comments/1")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "deleteContent".into()))
        .with_status(200)
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &["lpcli", "bug", "delete-comment", "-b", "42", "-c", "1"],
        )
        .await
        .unwrap(),
    );
    assert_eq!(plain, "OK: Comment #1 deleted from bug #42.\n");

    // 3. create (with attachment partial failure to verify Warning prefix)
    server
        .mock("POST", "/bugs")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "createBug".into()))
        .with_status(201)
        .with_header("location", &format!("{base}/bugs/42"))
        .create_async()
        .await;
    get(&mut server, "/bugs/42", &bug_json(&base, &[])).await;
    server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::Regex("addAttachment".into()))
        .with_status(500)
        .create_async()
        .await;
    let create_outcome = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "create",
            "-T",
            "title",
            "-d",
            "desc",
            "-u",
            "https://example.org/log",
        ],
    )
    .await
    .unwrap();
    let plain = render_plain(&create_outcome);
    assert_eq!(
        plain,
        "OK: Bug #42 created.\nURL: https://bugs.launchpad.net/bugs/42\n"
    );
    let warnings = create_outcome.render_warnings(&Output::fixed(OutputMode::Plain, false, 100));
    assert_plain_invariants(&warnings);
    assert!(warnings.starts_with("Warning: Failed: "), "{warnings}");
    assert!(!warnings.contains('✗'), "{warnings}");

    // 4. set-description
    server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"description": "updated"})))
        .with_body(bug_json(&base, &[]).to_string())
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &[
                "lpcli",
                "bug",
                "set-description",
                "-b",
                "42",
                "-d",
                "updated",
            ],
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        plain,
        "OK: Description updated for bug #42.\nURL: https://bugs.launchpad.net/bugs/42\n"
    );

    // 5-7. set-status, set-importance, set-assignee
    let base_task = task_json(&base, None, "New", None);
    let noble_task = task_json(&base, Some("noble"), "New", None);
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![base_task.clone(), noble_task.clone()]),
    )
    .await;
    server
        .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
        .with_body(base_task.to_string())
        .create_async()
        .await;
    server
        .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
        .with_body(noble_task.to_string())
        .create_async()
        .await;
    for (args, prefix) in [
        (
            vec![
                "lpcli",
                "bug",
                "set-status",
                "-b",
                "42",
                "--all-targets",
                "--all-series",
                "-s",
                "confirmed",
            ],
            "OK: Status updated to 'Confirmed' for 2 task(s) on bug #42.",
        ),
        (
            vec![
                "lpcli",
                "bug",
                "set-importance",
                "-b",
                "42",
                "-t",
                "curl",
                "--no-series",
                "-i",
                "high",
            ],
            "OK: Importance updated to 'High' for 1 task(s) on bug #42.",
        ),
        (
            vec![
                "lpcli",
                "bug",
                "set-assignee",
                "-b",
                "42",
                "-t",
                "curl",
                "--series",
                "noble",
                "-n",
                "someone",
            ],
            "OK: Assigned to ~someone for 1 task(s) on bug #42.",
        ),
    ] {
        let plain = render_plain(&run_bug(&server, &args).await.unwrap());
        assert_eq!(plain, format!("{prefix}\n"));
    }

    // 8-9. subscribe and unsubscribe
    server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "subscribe".into()))
        .with_status(200)
        .with_header("location", &format!("{base}/bugs/42/+subscription/someone"))
        .with_body(subscription_json(&base, "someone").to_string())
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &["lpcli", "bug", "subscribe", "-b", "42", "-n", "someone"],
        )
        .await
        .unwrap(),
    );
    assert!(
        plain.starts_with("OK: Subscribed ~someone to bug #42.\n"),
        "{plain}"
    );

    server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "unsubscribe".into()))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &["lpcli", "bug", "unsubscribe", "-b", "42", "-n", "someone"],
        )
        .await
        .unwrap(),
    );
    assert_eq!(plain, "OK: Unsubscribed ~someone from bug #42.\n");

    // 10. tag
    let updated_bug = bug_json(&base, &["regression"]);
    server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"tags": ["regression"]})))
        .with_body(updated_bug.to_string())
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &["lpcli", "bug", "tag", "-b", "42", "--add-tag", "regression"],
        )
        .await
        .unwrap(),
    );
    assert_eq!(plain, "OK: Bug #42 tags updated: regression\n");

    // 11. add-task
    server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "addTask".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/ubuntu/noble/+source/curl")),
        ]))
        .with_status(201)
        .with_header(
            "location",
            &format!("{base}/ubuntu/noble/+source/curl/+bug/42"),
        )
        .create_async()
        .await;
    get(
        &mut server,
        "/ubuntu/noble/+source/curl/+bug/42",
        &noble_task,
    )
    .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &[
                "lpcli", "bug", "add-task", "-b", "42", "-p", "curl", "--series", "noble",
            ],
        )
        .await
        .unwrap(),
    );
    assert_eq!(plain, "OK: Bug task added: curl (Ubuntu noble)\n");

    // 12. delete-task
    server
        .mock("DELETE", "/ubuntu/noble/+source/curl/+bug/42")
        .with_status(200)
        .create_async()
        .await;
    let plain = render_plain(
        &run_bug(
            &server,
            &[
                "lpcli",
                "bug",
                "delete-task",
                "-b",
                "42",
                "-p",
                "curl",
                "--series",
                "noble",
            ],
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        plain,
        "OK: Deleted bug task: curl (Ubuntu noble)\n\nOK: Deleted 1 task(s) on bug #42.\n"
    );
}

#[tokio::test]
async fn bug_remote_links_and_tracker_create_plain_use_word_prefixes() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let task = task_json(&base, None, "New", None);
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![task.clone()]),
    )
    .await;
    get(&mut server, "/ubuntu/+source/curl/+bug/42", &task).await;

    let tracker = json!({
        "self_link": format!("{base}/bugs/bugtrackers/debbugs"),
        "name": "debbugs",
        "title": "Debian Bug tracker",
        "bug_tracker_type": "Debbugs",
        "base_url": "https://bugs.debian.org/",
    });
    server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "queryByBaseURL".into()),
            Matcher::UrlEncoded("base_url".into(), "https://bugs.debian.org/".into()),
        ]))
        .with_body(tracker.to_string())
        .create_async()
        .await;
    server
        .mock("POST", "/bugs/bugtrackers")
        .match_body(Matcher::UrlEncoded(
            "ws.op".into(),
            "ensureBugTracker".into(),
        ))
        .with_status(200)
        .with_body(tracker.to_string())
        .create_async()
        .await;
    let tracker_outcome = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "tracker",
            "create",
            "--base-url",
            "https://bugs.debian.org/",
            "--type",
            "debbugs",
        ],
    )
    .await
    .unwrap();
    let tracker_plain = render_plain(&tracker_outcome);
    assert!(
        tracker_plain.starts_with("OK: Bug tracker 'debbugs' "),
        "{tracker_plain}"
    );
    assert!(tracker_plain.contains("Type: Debbugs\n"), "{tracker_plain}");

    // link-remote (using explicit debbugs tracker)
    server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getByName".into()),
            Matcher::UrlEncoded("name".into(), "debbugs".into()),
        ]))
        .with_body(tracker.to_string())
        .create_async()
        .await;
    let watch = watch_json(&base, 7, "1150409");
    get(
        &mut server,
        "/bugs/42/bug_watches",
        &collection(vec![watch]),
    )
    .await;
    let mut linked_task = task.clone();
    linked_task["bug_watch_link"] = format!("{base}/bugs/42/+watch/7").into();
    server
        .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
        .with_body(linked_task.to_string())
        .create_async()
        .await;
    let link_plain = render_plain(
        &run_bug(
            &server,
            &[
                "lpcli",
                "bug",
                "link-remote",
                "-b",
                "42",
                "--bug-tracker",
                "debbugs",
                "--remote-bug",
                "1150409",
            ],
        )
        .await
        .unwrap(),
    );
    assert!(
        link_plain.starts_with("OK: Linked task 'curl (Ubuntu)' to remote bug: "),
        "{link_plain}"
    );
    assert!(
        link_plain.contains("URL: https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=1150409\n"),
        "{link_plain}"
    );

    // unlink-remote when task already had a bug watch
    let mut watched_server = isolated_mock_server().await;
    let watched_base = watched_server.url();
    let mut watched_task = task_json(&watched_base, None, "New", None);
    watched_task["bug_watch_link"] = format!("{watched_base}/bugs/42/+watch/7").into();
    get(
        &mut watched_server,
        "/bugs/42/bug_tasks",
        &collection(vec![watched_task]),
    )
    .await;
    watched_server
        .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
        .with_body(task_json(&watched_base, None, "New", None).to_string())
        .create_async()
        .await;
    let unlink_plain = render_plain(
        &run_bug(
            &watched_server,
            &["lpcli", "bug", "unlink-remote", "-b", "42"],
        )
        .await
        .unwrap(),
    );
    assert!(
        unlink_plain.contains("OK: Unlinked remote bug watch from task 'curl (Ubuntu)'."),
        "{unlink_plain}"
    );
    assert!(
        unlink_plain.contains("The watch itself remains on bug #42"),
        "{unlink_plain}"
    );
}

#[tokio::test]
async fn partial_task_failure_warning_is_plain_in_plain_mode() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    get(
        &mut server,
        "/bugs/42/bug_tasks",
        &collection(vec![
            task_json(&base, None, "New", None),
            task_json(&base, Some("noble"), "New", None),
        ]),
    )
    .await;
    server
        .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
        .with_body(task_json(&base, None, "Triaged", None).to_string())
        .create_async()
        .await;
    server
        .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
        .with_status(403)
        .with_body("not allowed")
        .create_async()
        .await;
    let outcome = run_bug(
        &server,
        &[
            "lpcli",
            "bug",
            "set-status",
            "-b",
            "42",
            "--all-targets",
            "--all-series",
            "-s",
            "triaged",
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome.error.as_ref().unwrap().kind(), ErrorKind::Partial);
    let plain = render_plain(&outcome);
    assert_eq!(
        plain,
        "OK: Status updated to 'Triaged' for 1 task(s) on bug #42.\n"
    );
    let warnings = outcome.render_warnings(&Output::fixed(OutputMode::Plain, false, 100));
    assert_plain_invariants(&warnings);
    assert!(
        warnings.starts_with("Warning: Failed: set_status"),
        "{warnings}"
    );
    assert!(!warnings.contains('✗'), "{warnings}");
}

// ---------------------------------------------------------------------------
// `--progress`, accessible resolution, `NO_COLOR`, and `TERM=dumb`
// ---------------------------------------------------------------------------

#[test]
fn cli_progress_flag_parses_every_mode_and_resolves_automatically() {
    for (flag, expected) in [
        ("auto", ProgressChoice::Auto),
        ("plain", ProgressChoice::Plain),
        ("rich", ProgressChoice::Rich),
        ("none", ProgressChoice::None),
    ] {
        let cli = Cli::try_parse_from(["lpcli", "--progress", flag, "status"]).unwrap();
        validate_cli(&cli).unwrap();
        assert_eq!(cli.progress, expected, "--progress {flag}");
    }
    assert!(Cli::try_parse_from(["lpcli", "--progress", "verbose", "status"]).is_err());

    use output::{Environment, Settings};
    let resolve = |format: Option<OutputMode>,
                   accessible: bool,
                   env: &Environment,
                   stderr_terminal: bool,
                   progress: ProgressChoice| {
        Output::resolve(Settings {
            format,
            color: ColorChoice::Auto,
            config_format: None,
            accessible,
            display: Display::from_width(80),
            json_output: true,
            tsv_output: true,
            fields: None,
            env,
            stdout_terminal: true,
            stderr_terminal,
        })
        .unwrap()
        .with_progress(progress, stderr_terminal)
    };

    let default_env = Environment::default();
    assert_eq!(
        resolve(None, false, &default_env, true, ProgressChoice::Auto).progress(),
        download::ProgressMode::Rich
    );
    assert_eq!(
        resolve(None, false, &default_env, false, ProgressChoice::Auto).progress(),
        download::ProgressMode::Plain
    );
    assert_eq!(
        resolve(None, true, &default_env, true, ProgressChoice::Auto).progress(),
        download::ProgressMode::Plain
    );
    assert_eq!(
        resolve(
            Some(OutputMode::Json),
            false,
            &default_env,
            true,
            ProgressChoice::Auto
        )
        .progress(),
        download::ProgressMode::None
    );
    assert_eq!(
        resolve(
            Some(OutputMode::Tsv),
            false,
            &default_env,
            true,
            ProgressChoice::Auto
        )
        .progress(),
        download::ProgressMode::None
    );
    assert_eq!(
        resolve(None, false, &default_env, true, ProgressChoice::None).progress(),
        download::ProgressMode::None
    );
    assert_eq!(
        resolve(None, false, &default_env, true, ProgressChoice::Plain).progress(),
        download::ProgressMode::Plain
    );
    assert_eq!(
        resolve(
            Some(OutputMode::Plain),
            true,
            &default_env,
            true,
            ProgressChoice::Rich
        )
        .progress(),
        download::ProgressMode::Rich
    );

    let dumb = Environment {
        term: Some("dumb".into()),
        ..Environment::default()
    };
    let dumb_out = resolve(None, false, &dumb, true, ProgressChoice::Auto);
    assert_eq!(dumb_out.mode(), OutputMode::Plain);
    assert!(!dumb_out.stdout_style().color);
    assert_eq!(dumb_out.progress(), download::ProgressMode::Plain);

    let no_color = Environment {
        no_color: Some("1".into()),
        ..Environment::default()
    };
    let no_color_out = resolve(None, false, &no_color, true, ProgressChoice::Auto);
    assert_eq!(no_color_out.mode(), OutputMode::Rich);
    assert!(!no_color_out.stdout_style().color);
}

// ---------------------------------------------------------------------------
// Plain download progress formatters and runner
// ---------------------------------------------------------------------------

#[test]
fn plain_download_progress_lines_pass_plain_invariants() {
    let dir = std::path::Path::new("/tmp/out");
    let start = download::format_plain_start(2, dir);
    assert_plain_invariants(&start);
    assert_eq!(start, "Downloading 2 file(s) to '/tmp/out'...");

    let milestone = download::format_plain_milestone("curl_8.5.0.orig.tar.gz", 512, 1024);
    assert_plain_invariants(&milestone);
    assert_eq!(
        milestone,
        "Progress: curl_8.5.0.orig.tar.gz: 50% (512/1024 bytes)"
    );

    let ok = download::DownloadOutcome::Success(download::DownloadedFile {
        path: dir.join("curl_8.5.0.dsc"),
        size: 128,
    });
    let err = download::DownloadOutcome::Failed {
        filename: "curl_8.5.0.orig.tar.gz".into(),
        error: "HTTP 404 — Not found ✓ ✗ ─".into(),
    };
    let ok_line = download::format_plain_outcome(&ok);
    let err_line = download::format_plain_outcome(&err);
    assert_plain_invariants(&ok_line);
    assert_plain_invariants(&err_line);
    assert_eq!(ok_line, "OK: curl_8.5.0.dsc (128 bytes)");
    assert_eq!(
        err_line,
        "Error: curl_8.5.0.orig.tar.gz: HTTP 404 - Not found OK Error "
    );

    let ok_summary = download::format_plain_summary(std::slice::from_ref(&ok));
    let mixed_summary = download::format_plain_summary(&[ok, err]);
    assert_plain_invariants(&ok_summary);
    assert_plain_invariants(&mixed_summary);
    assert_eq!(ok_summary, "Download complete: 1 file(s) downloaded.");
    assert_eq!(
        mixed_summary,
        "Download complete: 1 file(s) downloaded, 1 failed."
    );
}

#[tokio::test]
async fn download_files_with_progress_plain_and_none_modes_download_all_files() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    server
        .mock("GET", "/files/a.txt")
        .with_body("alpha")
        .create_async()
        .await;
    server
        .mock("GET", "/files/b.txt")
        .with_status(404)
        .with_body("Missing")
        .create_async()
        .await;
    let client = LaunchpadClient::new(None).with_base_url(&base);
    let urls = vec![format!("{base}/files/a.txt"), format!("{base}/files/b.txt")];

    for mode in [download::ProgressMode::Plain, download::ProgressMode::None] {
        let dir = tempfile::tempdir().unwrap();
        let outcomes = download::download_files_with_progress(&client, &urls, dir.path(), mode)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(outcomes[0], download::DownloadOutcome::Success(_)));
        assert!(matches!(
            outcomes[1],
            download::DownloadOutcome::Failed { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "alpha"
        );
        assert!(!dir.path().join("b.txt").exists());
    }
}

// ---------------------------------------------------------------------------
// Character sanitization and prompt text
// ---------------------------------------------------------------------------

#[test]
fn clean_plain_neutralizes_banned_symbols_controls_and_emoji() {
    let dirty = "Header ─ ┌─┐ ✓ OK ✗ Err — em – en 📎🚀\t\r\n\u{1b}[31mred";
    let cleaned = output::clean_plain(dirty);
    assert_plain_invariants(&cleaned);
    assert!(!cleaned.contains('\r'));
    assert!(!cleaned.contains('\t'));
    assert!(!cleaned.contains('—'));
    assert!(!cleaned.contains('–'));
    assert!(!cleaned.contains('✓'));
    assert!(!cleaned.contains('✗'));
    assert!(!cleaned.contains('📎'));
    assert!(cleaned.contains("OK OK Error Err - em - en"));

    let out = Output::fixed(OutputMode::Plain, false, 80);
    let err = out.render_error_message("Failed — ✓ ✗ 📎\tbad\u{1b}[31m");
    let warn = out.render_warning("Watch — ✓ ✗ 📎\tbad\u{1b}[31m");
    assert_plain_invariants(&err);
    assert_plain_invariants(&warn);
    assert!(err.starts_with("Error: "), "{err}");
    assert!(warn.starts_with("Warning: "), "{warn}");
}

#[test]
fn hardware_key_choice_prompt_is_numbered_and_plain() {
    let prompt = hardware_key::format_choice_prompt(
        "security key",
        &["YubiKey 5".to_string(), "SoloKey".to_string()],
    );
    assert_plain_invariants(&prompt);
    assert!(prompt.contains("1. YubiKey 5\n"), "{prompt}");
    assert!(prompt.contains("2. SoloKey\n"), "{prompt}");
    assert!(
        prompt.contains("Choose a security key (enter a number from 1 to 2): "),
        "{prompt}"
    );
    assert!(!prompt.contains('['), "{prompt}");
    assert!(!prompt.contains(']'), "{prompt}");
}
