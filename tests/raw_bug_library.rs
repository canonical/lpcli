//! Mocked-HTTP tests for the `_raw` bug APIs in `lpcli::bugs`.
//!
//! The raw functions must return Launchpad's JSON unchanged (unknown fields,
//! `null`s, links, timestamps and enum display strings), use Launchpad's
//! collection shape for lists, and report mutations that Launchpad already
//! applied but that could not be confirmed as `LpError::Partial`.

use lpcli::bugs::{
    self, AddAttachmentParams, BugImportance, BugTaskStatus, EnsureBugTrackerParams,
};
use lpcli::client::LaunchpadClient;
use lpcli::error::LpError;
use mockito::{Matcher, Mock, Server, ServerGuard};
use serde_json::{Value, json};

fn client(server: &ServerGuard) -> LaunchpadClient {
    LaunchpadClient::new(None).with_base_url(server.url())
}

async fn mock_json(server: &mut ServerGuard, method: &str, path: &str, body: &Value) -> Mock {
    server
        .mock(method, path)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await
}

async fn mock_created(server: &mut ServerGuard, path: &str, location: &str) -> Mock {
    server
        .mock("POST", path)
        .with_status(201)
        .with_header("Location", location)
        .expect(1)
        .create_async()
        .await
}

async fn mock_never(server: &mut ServerGuard, method: &str, path: &str) -> Mock {
    server
        .mock(method, path)
        .match_query(Matcher::Any)
        .expect(0)
        .create_async()
        .await
}

async fn assert_all(mocks: &[Mock]) {
    for mock in mocks {
        mock.assert_async().await;
    }
}

fn assert_partial(result: Result<impl std::fmt::Debug, LpError>, fragments: &[&str]) {
    match result {
        Err(LpError::Partial(message)) => {
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "missing '{fragment}' in: {message}"
                );
            }
        }
        other => panic!("expected LpError::Partial, got {other:?}"),
    }
}

fn assert_usage(result: Result<impl std::fmt::Debug, LpError>, fragment: &str) {
    match result {
        Err(LpError::Usage(message)) => {
            assert!(
                message.contains(fragment),
                "missing '{fragment}' in: {message}"
            );
        }
        other => panic!("expected LpError::Usage, got {other:?}"),
    }
}

fn bug(base: &str, id: u64) -> Value {
    json!({
        "id": id,
        "title": "Crash on start",
        "description": "It crashes.",
        "tags": ["regression"],
        "date_created": "2026-01-02T03:04:05.123456+00:00",
        "date_last_updated": null,
        "self_link": format!("{base}/bugs/{id}"),
        "web_link": format!("https://bugs.launchpad.net/bugs/{id}"),
        "owner_link": format!("{base}/~reporter"),
        "users_affected_count": 3,
        "resource_type_link": format!("{base}/#bug"),
        "http_etag": "\"etag-1\"",
        "heat": 42,
        "private": false,
        "duplicate_of_link": null
    })
}

fn task(base: &str, status: &str) -> Value {
    json!({
        "self_link": format!("{base}/ubuntu/+source/apport/+bug/42"),
        "bug_link": format!("{base}/bugs/42"),
        "title": "Bug #42 in apport (Ubuntu): \"Crash on start\"",
        "status": status,
        "importance": "Undecided",
        "assignee_link": null,
        "date_created": "2026-01-02T03:04:05.123456+00:00",
        "bug_target_display_name": "apport (Ubuntu)",
        "target_link": format!("{base}/ubuntu/+source/apport"),
        "bug_watch_link": null,
        "resource_type_link": format!("{base}/#bug_task"),
        "http_etag": "\"task-etag\"",
        "is_complete": false
    })
}

fn tracker(base: &str, name: &str, kind: &str, base_url: &str) -> Value {
    json!({
        "self_link": format!("{base}/bugs/bugtrackers/{name}"),
        "name": name,
        "title": "A tracker",
        "summary": null,
        "base_url": base_url,
        "bug_tracker_type": kind,
        "web_link": format!("https://bugs.launchpad.net/bugs/bugtrackers/{name}"),
        "resource_type_link": format!("{base}/#bug_tracker"),
        "active": true
    })
}

fn watch(base: &str, id: u64, tracker_name: &str, remote: &str, url: &str) -> Value {
    json!({
        "self_link": format!("{base}/bugs/42/+watch/{id}"),
        "bug_link": format!("{base}/bugs/42"),
        "bug_tracker_link": format!("{base}/bugs/bugtrackers/{tracker_name}"),
        "remote_bug": remote,
        "title": format!("{tracker_name} #{remote}"),
        "url": url,
        "web_link": format!("https://bugs.launchpad.net/bugs/42/+watch/{id}"),
        "remote_status": null,
        "remote_importance": null,
        "date_created": "2026-01-02T03:04:05+00:00",
        "date_last_checked": null,
        "date_last_changed": null,
        "owner_link": format!("{base}/~reporter"),
        "last_error_type": null,
        "resource_type_link": format!("{base}/#bug_watch")
    })
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_bug_raw_returns_exact_launchpad_entry() {
    let mut server = Server::new_async().await;
    let body = bug(&server.url(), 42);
    let get = mock_json(&mut server, "GET", "/bugs/42", &body).await;

    let raw = bugs::get_bug_raw(&client(&server), 42).await.unwrap();

    assert_eq!(raw.raw, body);
    assert_eq!(raw.value.id, 42);
    assert_eq!(raw.value.tags, vec!["regression"]);
    get.assert_async().await;
}

#[tokio::test]
async fn bug_collection_raw_reads_merge_pages_and_keep_collection_shape() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let first = |path: &str, entry: Value| {
        json!({
            "total_size": 2,
            "start": 0,
            "entries": [entry],
            "next_collection_link": format!("{base}/bugs/42/{path}?ws.start=1&ws.size=1"),
            "resource_type_link": format!("{base}/#page-resource")
        })
    };
    let second = |entry: Value| json!({"start": 1, "entries": [entry]});
    let comment = |index: u64| {
        json!({
            "self_link": format!("{base}/bugs/42/comments/{index}"),
            "content": format!("comment {index}"),
            "date_created": "2026-01-02T03:04:05+00:00",
            "date_deleted": null,
            "owner_link": format!("{base}/~someone"),
            "index": index,
            "subject": "Re: Crash"
        })
    };
    let attachment = |n: u64| {
        json!({
            "self_link": format!("{base}/bugs/42/+attachment/{n}"),
            "title": format!("log-{n}.txt"),
            "type": "Unspecified",
            "bug_link": format!("{base}/bugs/42"),
            "data_link": format!("{base}/bugs/42/+attachment/{n}/data"),
            "message_link": format!("{base}/bugs/42/comments/{n}"),
            "web_link": null
        })
    };
    let subscription = |name: &str| {
        json!({
            "self_link": format!("{base}/bugs/42/+subscription/{name}"),
            "person_link": format!("{base}/~{name}"),
            "bug_link": format!("{base}/bugs/42"),
            "date_created": "2026-01-02T03:04:05+00:00",
            "subscribed_by_link": format!("{base}/~{name}"),
            "bug_notification_level": "Discussion"
        })
    };
    let cases = [
        ("bug_tasks", task(&base, "New"), task(&base, "Confirmed")),
        ("messages", comment(0), comment(1)),
        ("attachments", attachment(1), attachment(2)),
        ("subscriptions", subscription("a"), subscription("b")),
        (
            "bug_watches",
            watch(&base, 1, "debbugs", "100", "https://bugs.debian.org/100"),
            watch(&base, 2, "debbugs", "200", "https://bugs.debian.org/200"),
        ),
    ];

    let mut mocks = Vec::new();
    for (path, entry_a, entry_b) in &cases {
        mocks.push(
            mock_json(
                &mut server,
                "GET",
                &format!("/bugs/42/{path}"),
                &first(path, entry_a.clone()),
            )
            .await,
        );
        mocks.push(
            server
                .mock("GET", format!("/bugs/42/{path}").as_str())
                .match_query(Matcher::AllOf(vec![
                    Matcher::UrlEncoded("ws.start".into(), "1".into()),
                    Matcher::UrlEncoded("ws.size".into(), "1".into()),
                ]))
                .with_header("content-type", "application/json")
                .with_body(second(entry_b.clone()).to_string())
                .expect(1)
                .create_async()
                .await,
        );
    }

    let client = client(&server);
    let raws = [
        bugs::get_bug_tasks_raw(&client, 42).await.unwrap().raw,
        bugs::get_bug_comments_raw(&client, 42).await.unwrap().raw,
        bugs::get_bug_attachments_raw(&client, 42)
            .await
            .unwrap()
            .raw,
        bugs::get_bug_subscriptions_raw(&client, 42)
            .await
            .unwrap()
            .raw,
        bugs::get_bug_watches_raw(&client, 42).await.unwrap().raw,
    ];

    for ((_, entry_a, entry_b), raw) in cases.iter().zip(raws) {
        assert_eq!(
            raw,
            json!({"total_size": 2, "start": 0, "entries": [entry_a, entry_b]}),
            "every page is merged and next_collection_link is omitted"
        );
    }
    assert_all(&mocks).await;
}

#[tokio::test]
async fn bug_collection_raw_passes_total_size_link_through() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let page = json!({
        "total_size_link": format!("{base}/bugs/42/bug_tasks?ws.show=total_size"),
        "start": 0,
        "entries": [task(&base, "New")]
    });
    let get = mock_json(&mut server, "GET", "/bugs/42/bug_tasks", &page).await;

    let raw = bugs::get_bug_tasks_raw(&client(&server), 42).await.unwrap();

    assert_eq!(raw.raw["total_size_link"], page["total_size_link"]);
    assert!(raw.raw.get("total_size").is_none(), "no invented count");
    assert_eq!(raw.raw["entries"], page["entries"]);
    get.assert_async().await;
}

#[tokio::test]
async fn tracker_and_watch_raw_reads_return_exact_entries() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_body = tracker(&base, "debbugs", "Debbugs", "https://bugs.debian.org/");
    let watch_body = watch(&base, 7, "debbugs", "100", "https://bugs.debian.org/100");
    let by_name = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getByName".into()),
            Matcher::UrlEncoded("name".into(), "debbugs".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(tracker_body.to_string())
        .expect(1)
        .create_async()
        .await;
    let missing = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getByName".into()),
            Matcher::UrlEncoded("name".into(), "missing".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body("null")
        .expect(1)
        .create_async()
        .await;
    let by_link = mock_json(
        &mut server,
        "GET",
        "/bugs/bugtrackers/debbugs",
        &tracker_body,
    )
    .await;
    let get_watch = mock_json(&mut server, "GET", "/bugs/42/+watch/7", &watch_body).await;
    let client = client(&server);

    let named = bugs::get_bug_tracker_by_name_raw(&client, "debbugs")
        .await
        .unwrap();
    assert_eq!(named.raw, tracker_body);
    assert!(matches!(
        bugs::get_bug_tracker_by_name_raw(&client, "missing").await,
        Err(LpError::LocalNotFound(_))
    ));
    let linked = bugs::get_bug_tracker_raw(&client, &format!("{base}/bugs/bugtrackers/debbugs"))
        .await
        .unwrap();
    assert_eq!(linked.raw, tracker_body);
    let watch_raw = bugs::get_bug_watch_raw(&client, &format!("{base}/bugs/42/+watch/7"))
        .await
        .unwrap();
    assert_eq!(watch_raw.raw, watch_body);
    assert_eq!(watch_raw.value.id(), Some("7"));
    assert_all(&[by_name, missing, by_link, get_watch]).await;
}

#[tokio::test]
async fn tracker_search_zero_limit_is_a_usage_error() {
    let server = Server::new_async().await;
    let result = bugs::search_bug_trackers_raw(&client(&server), None, Some(0)).await;
    assert_usage(result, "limit must be positive");
}

// ---------------------------------------------------------------------------
// Created entries: follow Location, read back exactly, Partial on failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_bug_raw_reads_back_the_created_entry() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = bug(&base, 77);
    let post = server
        .mock("POST", "/bugs")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "createBug".into()),
            Matcher::UrlEncoded("title".into(), "Crash on start".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/apport")),
        ]))
        .with_status(201)
        .with_header("Location", &format!("{base}/bugs/77"))
        .expect(1)
        .create_async()
        .await;
    let get = mock_json(&mut server, "GET", "/bugs/77", &body).await;

    let raw = bugs::create_bug_raw(&client(&server), "apport", "Crash on start", "It crashes.")
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn create_bug_raw_read_back_failure_is_partial() {
    let mut server = Server::new_async().await;
    let location = format!("{}/bugs/77", server.url());
    let post = mock_created(&mut server, "/bugs", &location).await;
    let get = server
        .mock("GET", "/bugs/77")
        .with_status(500)
        .with_body("read failed")
        .expect(1)
        .create_async()
        .await;

    let result = bugs::create_bug_raw(&client(&server), "apport", "Crash on start", "x").await;

    assert_partial(
        result,
        &[
            "Filing the bug succeeded",
            &location,
            "read failed",
            "may have been filed",
            "duplicate",
        ],
    );
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn create_bug_raw_missing_or_foreign_location_is_partial_and_not_fetched() {
    let mut server = Server::new_async().await;
    let post = server
        .mock("POST", "/bugs")
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let result = bugs::create_bug_raw(&client(&server), "apport", "Crash", "x").await;
    assert_partial(result, &["Location", "may have been filed"]);
    post.assert_async().await;

    let mut foreign = Server::new_async().await;
    let foreign_get = mock_never(&mut foreign, "GET", "/bugs/77").await;
    let mut server = Server::new_async().await;
    let post = mock_created(&mut server, "/bugs", &format!("{}/bugs/77", foreign.url())).await;
    let result = bugs::create_bug_raw(&client(&server), "apport", "Crash", "x").await;
    assert_partial(
        result,
        &["not on the configured API origin", "may have been filed"],
    );
    assert_all(&[post, foreign_get]).await;
}

#[tokio::test]
async fn create_bug_raw_rejected_post_is_not_partial() {
    let mut server = Server::new_async().await;
    let post = server
        .mock("POST", "/bugs")
        .with_status(400)
        .with_body("bad target")
        .expect(1)
        .create_async()
        .await;
    let result = bugs::create_bug_raw(&client(&server), "nope", "Crash", "x").await;
    assert!(
        matches!(result, Err(LpError::Api { status: 400, .. })),
        "{result:?}"
    );
    post.assert_async().await;
}

#[tokio::test]
async fn add_bug_task_raw_reads_back_and_reports_partial() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = task(&base, "New");
    let location = format!("{base}/ubuntu/+source/apport/+bug/42");
    let target = format!("{base}/ubuntu/+source/apport");
    let post = server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "addTask".into()),
            Matcher::UrlEncoded("target".into(), target.clone()),
        ]))
        .with_status(201)
        .with_header("Location", &location)
        .expect(1)
        .create_async()
        .await;
    let get = mock_json(&mut server, "GET", "/ubuntu/+source/apport/+bug/42", &body).await;
    let raw = bugs::add_bug_task_raw(&client(&server), 42, &target)
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    assert_all(&[post, get]).await;

    let mut server = Server::new_async().await;
    let location = format!("{}/ubuntu/+source/apport/+bug/42", server.url());
    let post = mock_created(&mut server, "/bugs/42", &location).await;
    let get = server
        .mock("GET", "/ubuntu/+source/apport/+bug/42")
        .with_body("{not json")
        .expect(1)
        .create_async()
        .await;
    let result = bugs::add_bug_task_raw(&client(&server), 42, &target).await;
    assert_partial(
        result,
        &["Adding the bug task succeeded", "lpcli bug tasks -b 42"],
    );
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn add_bug_comment_raw_follows_location() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = json!({
        "self_link": format!("{base}/bugs/42/comments/3"),
        "content": "Thanks!",
        "date_created": "2026-01-02T03:04:05.5+00:00",
        "date_deleted": null,
        "owner_link": format!("{base}/~me"),
        "index": 3,
        "subject": "Re: Crash on start",
        "bug_attachments_collection_link": format!("{base}/bugs/42/comments/3/bug_attachments")
    });
    let post = server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "newMessage".into()),
            Matcher::UrlEncoded("content".into(), "Thanks!".into()),
        ]))
        .with_status(201)
        .with_header("Location", &format!("{base}/bugs/42/comments/3"))
        .expect(1)
        .create_async()
        .await;
    let get = mock_json(&mut server, "GET", "/bugs/42/comments/3", &body).await;

    let raw = bugs::add_bug_comment_raw(&client(&server), 42, "Thanks!")
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    assert_eq!(raw.value.index, Some(3));
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn add_bug_comment_raw_without_location_is_partial() {
    let mut server = Server::new_async().await;
    let post = server
        .mock("POST", "/bugs/42")
        .with_status(201)
        .expect(1)
        .create_async()
        .await;
    let result = bugs::add_bug_comment_raw(&client(&server), 42, "Thanks!").await;
    assert_partial(result, &["Location", "lpcli bug comments -b 42"]);
    post.assert_async().await;
}

#[tokio::test]
async fn edit_bug_comment_raw_resolves_position_and_reads_back() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comment_url = format!("{base}/bugs/42/comments/2");
    // No `index` members (production shape), and a deleted comment before the
    // target: positions stay stable, so #2 is the third entry.
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0")},
        {"self_link": format!("{base}/bugs/42/comments/1"),
         "date_deleted": "2026-01-04T00:00:00+00:00"},
        {"self_link": comment_url}
    ]});
    let updated = json!({
        "self_link": comment_url,
        "content": "It's fixed: cannot reproduce.",
        "date_created": "2026-01-02T03:04:05.5+00:00",
        "date_deleted": null,
        "owner_link": format!("{base}/~me"),
    });
    let list = mock_json(&mut server, "GET", "/bugs/42/messages", &comments).await;
    let edit = server
        .mock("POST", "/bugs/42/comments/2")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "editContent".into()),
            Matcher::UrlEncoded("new_content".into(), "It's fixed: cannot reproduce.".into()),
        ]))
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let read_back = mock_json(&mut server, "GET", "/bugs/42/comments/2", &updated).await;

    let raw = bugs::edit_bug_comment_raw(&client(&server), 42, 2, "It's fixed: cannot reproduce.")
        .await
        .unwrap();

    assert_eq!(raw.raw, updated);
    assert_eq!(
        raw.value.content.as_deref(),
        Some("It's fixed: cannot reproduce.")
    );
    assert_eq!(raw.value.index, None);
    assert_all(&[list, edit, read_back]).await;
}

#[tokio::test]
async fn edit_bug_comment_raw_prefers_launchpad_index() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comment_url = format!("{base}/bugs/42/comments/7");
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0"), "index": 0},
        {"self_link": comment_url, "index": 7}
    ]});
    let updated = json!({"self_link": comment_url, "content": "fixed", "index": 7});
    let list = mock_json(&mut server, "GET", "/bugs/42/messages", &comments).await;
    let edit = server
        .mock("POST", "/bugs/42/comments/7")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "editContent".into()),
            Matcher::UrlEncoded("new_content".into(), "fixed".into()),
        ]))
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let read_back = mock_json(&mut server, "GET", "/bugs/42/comments/7", &updated).await;

    let raw = bugs::edit_bug_comment_raw(&client(&server), 42, 7, "fixed")
        .await
        .unwrap();

    assert_eq!(raw.value.index, Some(7));
    assert_all(&[list, edit, read_back]).await;
}

#[tokio::test]
async fn edit_bug_comment_raw_maps_403_to_unauthorized() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0")},
        {"self_link": format!("{base}/bugs/42/comments/1")}
    ]});
    let list = mock_json(&mut server, "GET", "/bugs/42/messages", &comments).await;
    let edit = server
        .mock("POST", "/bugs/42/comments/1")
        .with_status(403)
        .with_body("Only the author may edit this message.")
        .expect(1)
        .create_async()
        .await;

    let result = bugs::edit_bug_comment_raw(&client(&server), 42, 1, "fixed").await;

    match result {
        Err(LpError::Unauthorized(message)) => {
            assert!(
                message.contains("edited by its original author"),
                "{message}"
            );
            assert!(message.contains("Only the author may edit"), "{message}");
        }
        other => panic!("expected LpError::Unauthorized, got {other:?}"),
    }
    assert_all(&[list, edit]).await;
}

#[tokio::test]
async fn edit_bug_comment_raw_reports_partial_when_read_back_fails() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comment_url = format!("{base}/bugs/42/comments/1");
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0")},
        {"self_link": comment_url}
    ]});
    let list = mock_json(&mut server, "GET", "/bugs/42/messages", &comments).await;
    let edit = server
        .mock("POST", "/bugs/42/comments/1")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let read_back = server
        .mock("GET", "/bugs/42/comments/1")
        .with_body("{not json")
        .expect(1)
        .create_async()
        .await;

    let result = bugs::edit_bug_comment_raw(&client(&server), 42, 1, "fixed").await;

    assert_partial(
        result,
        &[
            "Editing comment #1 on bug #42 succeeded",
            "new text was applied",
            "lpcli bug comments -b 42",
        ],
    );
    assert_all(&[list, edit, read_back]).await;
}

#[tokio::test]
async fn edit_bug_comment_raw_refuses_deleted_or_unknown_comments() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0")},
        {"self_link": format!("{base}/bugs/42/comments/1"),
         "date_deleted": "2026-01-04T00:00:00+00:00"}
    ]});
    let list = server
        .mock("GET", "/bugs/42/messages")
        .with_header("content-type", "application/json")
        .with_body(comments.to_string())
        .expect(2)
        .create_async()
        .await;
    let never_deleted = mock_never(&mut server, "POST", "/bugs/42/comments/1").await;
    let never_unknown = mock_never(&mut server, "POST", "/bugs/42/comments/9").await;

    let deleted = bugs::edit_bug_comment_raw(&client(&server), 42, 1, "fixed").await;
    match deleted {
        Err(LpError::LocalNotFound(message)) => {
            assert!(
                message.contains("deleted and cannot be edited"),
                "{message}"
            );
        }
        other => panic!("expected LpError::LocalNotFound, got {other:?}"),
    }

    let unknown = bugs::edit_bug_comment_raw(&client(&server), 42, 9, "fixed").await;
    match unknown {
        Err(LpError::LocalNotFound(message)) => {
            assert!(
                message.contains("Comment #9 does not exist on bug #42"),
                "{message}"
            );
        }
        other => panic!("expected LpError::LocalNotFound, got {other:?}"),
    }
    list.assert_async().await;
    never_deleted.assert_async().await;
    never_unknown.assert_async().await;
}

#[tokio::test]
async fn add_bug_attachment_raw_reads_back_the_attachment() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = json!({
        "self_link": format!("{base}/bugs/42/+attachment/9"),
        "title": "fix.patch",
        "type": "Patch",
        "bug_link": format!("{base}/bugs/42"),
        "data_link": format!("{base}/bugs/42/+attachment/9/data"),
        "message_link": format!("{base}/bugs/42/comments/4"),
        "web_link": "https://bugs.launchpad.net/bugs/42/+attachment/9",
        "http_etag": "\"att\""
    });
    let post = mock_created(
        &mut server,
        "/bugs/42",
        &format!("{base}/bugs/42/+attachment/9"),
    )
    .await;
    let get = mock_json(&mut server, "GET", "/bugs/42/+attachment/9", &body).await;
    let params = AddAttachmentParams {
        comment: "patch",
        url: Some("https://example.com/fix.patch"),
        is_patch: true,
        ..Default::default()
    };

    let raw = bugs::add_bug_attachment_raw(&client(&server), 42, &params)
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    assert_eq!(raw.value.attachment_type.as_deref(), Some("Patch"));
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn add_bug_attachment_raw_validates_source_and_reports_partial() {
    let mut server = Server::new_async().await;
    let never = mock_never(&mut server, "POST", "/bugs/42").await;
    let path = std::path::Path::new("/nonexistent");
    for params in [
        AddAttachmentParams::default(),
        AddAttachmentParams {
            file_path: Some(path),
            url: Some("https://example.com/x"),
            ..Default::default()
        },
    ] {
        let result = bugs::add_bug_attachment_raw(&client(&server), 42, &params).await;
        assert_usage(result, "exactly one");
    }
    never.assert_async().await;

    let mut server = Server::new_async().await;
    let location = format!("{}/bugs/42/+attachment/9", server.url());
    let post = mock_created(&mut server, "/bugs/42", &location).await;
    let get = server
        .mock("GET", "/bugs/42/+attachment/9")
        .with_status(503)
        .with_body("librarian down")
        .expect(1)
        .create_async()
        .await;
    let params = AddAttachmentParams {
        url: Some("https://example.com/fix.patch"),
        ..Default::default()
    };
    let result = bugs::add_bug_attachment_raw(&client(&server), 42, &params).await;
    assert_partial(result, &["librarian down", "lpcli bug attachments -b 42"]);
    assert_all(&[post, get]).await;
}

#[tokio::test]
async fn subscribe_to_bug_raw_returns_exact_entry_and_partial_on_bad_body() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = json!({
        "self_link": format!("{base}/bugs/42/+subscription/me"),
        "person_link": format!("{base}/~me"),
        "bug_link": format!("{base}/bugs/42"),
        "date_created": "2026-01-02T03:04:05+00:00",
        "subscribed_by_link": format!("{base}/~me"),
        "bug_notification_level": "Discussion"
    });
    let post = server
        .mock("POST", "/bugs/42")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "subscribe".into()))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;
    let person = format!("{base}/~me");
    let raw = bugs::subscribe_to_bug_raw(&client(&server), 42, &person)
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    post.assert_async().await;

    let mut server = Server::new_async().await;
    let post = server
        .mock("POST", "/bugs/42")
        .with_body("<html>ok</html>")
        .expect(1)
        .create_async()
        .await;
    let result = bugs::subscribe_to_bug_raw(&client(&server), 42, &person).await;
    assert_partial(
        result,
        &[
            "Subscribing to the bug succeeded",
            "lpcli bug subscriptions -b 42",
        ],
    );
    post.assert_async().await;
}

// ---------------------------------------------------------------------------
// PATCH mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_patch_raw_mutations_send_json_and_return_exact_entry() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}/ubuntu/+source/apport/+bug/42");
    let assignee = format!("{base}/~dev");
    let watch_url = format!("{base}/bugs/42/+watch/7");
    let cases = [
        ("status", json!({"status": "Won't Fix"})),
        ("importance", json!({"importance": "High"})),
        ("assignee", json!({"assignee_link": assignee})),
        ("watch", json!({"bug_watch_link": watch_url})),
        ("unwatch", json!({"bug_watch_link": null})),
    ];
    let client = client(&server);
    for (case, expected_body) in cases {
        let response = task(&base, "Won't Fix");
        let patch = server
            .mock("PATCH", "/ubuntu/+source/apport/+bug/42")
            .match_header("content-type", "application/json")
            .match_body(Matcher::Json(expected_body))
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .expect(1)
            .create_async()
            .await;
        let raw = match case {
            "status" => bugs::set_bug_status_raw(&client, &task_url, &BugTaskStatus::WontFix).await,
            "importance" => {
                bugs::set_bug_importance_raw(&client, &task_url, &BugImportance::High).await
            }
            "assignee" => bugs::set_bug_assignee_raw(&client, &task_url, &assignee).await,
            "watch" => bugs::set_bug_watch_raw(&client, &task_url, &watch_url).await,
            _ => bugs::unset_bug_watch_raw(&client, &task_url).await,
        }
        .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(raw.raw, response, "{case}");
        patch.assert_async().await;
        patch.remove_async().await;
    }
}

#[tokio::test]
async fn bug_patch_raw_mutations_return_exact_entry() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let body = bug(&base, 42);
    let description = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"description": "New text"})))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;
    let tags = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"tags": ["a", "b"]})))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;
    let mark = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({
            "duplicate_of_link": format!("{base}/bugs/43"),
        })))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;
    let unmark = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"duplicate_of_link": null})))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;
    let client = client(&server);

    let raw = bugs::set_bug_description_raw(&client, 42, "New text")
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    let raw = bugs::set_bug_tags_raw(&client, 42, &["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    let raw = bugs::mark_bug_as_duplicate_raw(&client, 42, 43)
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    let raw = bugs::unmark_bug_as_duplicate_raw(&client, 42)
        .await
        .unwrap();
    assert_eq!(raw.raw, body);
    assert_all(&[description, tags, mark, unmark]).await;
}

#[tokio::test]
async fn mark_bug_as_duplicate_rejects_the_bug_itself_without_a_request() {
    let server = Server::new_async().await;
    let client = client(&server);
    let error = bugs::mark_bug_as_duplicate_raw(&client, 42, 42)
        .await
        .expect_err("a bug cannot be a duplicate of itself");
    assert!(matches!(error, LpError::Usage(_)), "{error}");
    let typed_error = bugs::mark_bug_as_duplicate(&client, 42, 42)
        .await
        .expect_err("a bug cannot be a duplicate of itself");
    assert!(matches!(typed_error, LpError::Usage(_)), "{typed_error}");
}

#[tokio::test]
async fn typed_duplicate_helpers_patch_bug_and_unreadable_raw_responses_are_partial() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let marked_body = json!({
        "id": 42,
        "title": "Sample bug",
        "tags": [],
        "duplicate_of_link": format!("{base}/bugs/43"),
        "number_of_duplicates": 0
    });
    let unmarked_body = json!({
        "id": 42,
        "title": "Sample bug",
        "tags": [],
        "duplicate_of_link": null,
        "number_of_duplicates": 0
    });
    let mark = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({
            "duplicate_of_link": format!("{base}/bugs/43"),
        })))
        .with_header("content-type", "application/json")
        .with_body(marked_body.to_string())
        .expect(1)
        .create_async()
        .await;
    let unmark = server
        .mock("PATCH", "/bugs/42")
        .match_body(Matcher::Json(json!({"duplicate_of_link": null})))
        .with_header("content-type", "application/json")
        .with_body(unmarked_body.to_string())
        .expect(1)
        .create_async()
        .await;
    let client_ok = client(&server);

    let marked = bugs::mark_bug_as_duplicate(&client_ok, 42, 43)
        .await
        .unwrap();
    assert_eq!(
        marked.duplicate_of_link.as_deref(),
        Some(format!("{base}/bugs/43").as_str())
    );
    let unmarked = bugs::unmark_bug_as_duplicate(&client_ok, 42).await.unwrap();
    assert_eq!(unmarked.duplicate_of_link, None);
    assert_all(&[mark, unmark]).await;

    let mut bad_server = Server::new_async().await;
    let bad_mark = bad_server
        .mock("PATCH", "/bugs/42")
        .with_body("not json")
        .expect(2)
        .create_async()
        .await;
    let bad_client = client(&bad_server);
    assert_partial(
        bugs::mark_bug_as_duplicate_raw(&bad_client, 42, 43).await,
        &[
            "Marking the bug as a duplicate succeeded",
            "lpcli bug show -b 42",
        ],
    );
    assert_partial(
        bugs::unmark_bug_as_duplicate_raw(&bad_client, 42).await,
        &[
            "Removing the duplicate marker succeeded",
            "lpcli bug show -b 42",
        ],
    );
    bad_mark.assert_async().await;
}

#[tokio::test]
async fn patch_raw_unreadable_success_is_partial_but_rejection_is_not() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}/ubuntu/+source/apport/+bug/42");
    let patch = server
        .mock("PATCH", "/ubuntu/+source/apport/+bug/42")
        .with_body("not json")
        .expect(1)
        .create_async()
        .await;
    let result =
        bugs::set_bug_status_raw(&client(&server), &task_url, &BugTaskStatus::Triaged).await;
    assert_partial(
        result,
        &["Setting the bug task status succeeded", &task_url],
    );
    patch.assert_async().await;

    let tags = server
        .mock("PATCH", "/bugs/42")
        .with_body("not json")
        .expect(1)
        .create_async()
        .await;
    let result = bugs::set_bug_tags_raw(&client(&server), 42, &[]).await;
    assert_partial(
        result,
        &["Setting the bug tags succeeded", "lpcli bug show -b 42"],
    );
    tags.assert_async().await;

    let mut server = Server::new_async().await;
    let patch = server
        .mock("PATCH", "/bugs/42")
        .with_status(401)
        .with_body("locked")
        .expect(1)
        .create_async()
        .await;
    let result = bugs::set_bug_description_raw(&client(&server), 42, "x").await;
    assert!(
        matches!(result, Err(LpError::Api { status: 401, .. })),
        "{result:?}"
    );
    patch.assert_async().await;
}

// ---------------------------------------------------------------------------
// Multi-step operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_bug_comment_after_removing_attachment_reports_partial() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let comment_url = format!("{base}/bugs/42/comments/1");
    let comments = json!({"entries": [
        {"self_link": format!("{base}/bugs/42/comments/0"), "index": 0},
        {"self_link": comment_url, "index": 1}
    ]});
    let attachments = json!({"entries": [{
        "self_link": format!("{base}/bugs/42/+attachment/5"),
        "message_link": comment_url
    }]});
    let list_comments = mock_json(&mut server, "GET", "/bugs/42/messages", &comments).await;
    let list_attachments =
        mock_json(&mut server, "GET", "/bugs/42/attachments", &attachments).await;
    let remove = server
        .mock("POST", "/bugs/42/+attachment/5")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "removeFromBug".into()))
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let delete = server
        .mock("POST", "/bugs/42/comments/1")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "deleteContent".into()))
        .with_status(500)
        .with_body("delete failed")
        .expect(1)
        .create_async()
        .await;

    let result = bugs::delete_bug_comment(&client(&server), 42, 1).await;

    assert_partial(
        result,
        &[
            "/bugs/42/+attachment/5",
            "delete failed",
            "comment text was not deleted",
            "-b 42",
        ],
    );
    assert_all(&[list_comments, list_attachments, remove, delete]).await;
}

const TASK_PATH: &str = "/ubuntu/+source/apport/+bug/42";

async fn mock_task(server: &mut ServerGuard) -> Mock {
    let base = server.url();
    mock_json(server, "GET", TASK_PATH, &task(&base, "New")).await
}

async fn mock_tracker_by_name(server: &mut ServerGuard, body: &Value) -> Mock {
    server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(Matcher::UrlEncoded("ws.op".into(), "getByName".into()))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await
}

async fn mock_watches(server: &mut ServerGuard, entries: &[Value]) -> Mock {
    let page = json!({"total_size": entries.len(), "start": 0, "entries": entries});
    mock_json(server, "GET", "/bugs/42/bug_watches", &page).await
}

#[tokio::test]
async fn link_bug_task_remote_raw_creates_watch_and_returns_exact_entries() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}{TASK_PATH}");
    let watch_body = watch(
        &base,
        101,
        "debbugs",
        "1150409",
        "https://bugs.debian.org/1150409",
    );
    let linked = {
        let mut linked = task(&base, "New");
        linked["bug_watch_link"] = json!(format!("{base}/bugs/42/+watch/101"));
        linked
    };
    let get_task = mock_task(&mut server).await;
    let tracker_mock = mock_tracker_by_name(
        &mut server,
        &tracker(&base, "debbugs", "Debbugs", "https://bugs.debian.org/"),
    )
    .await;
    let list = mock_watches(&mut server, &[]).await;
    let post = mock_created(
        &mut server,
        "/bugs/42",
        &format!("{base}/bugs/42/+watch/101"),
    )
    .await;
    let get_watch = mock_json(&mut server, "GET", "/bugs/42/+watch/101", &watch_body).await;
    let patch = server
        .mock("PATCH", TASK_PATH)
        .match_body(Matcher::Json(
            json!({"bug_watch_link": format!("{base}/bugs/42/+watch/101")}),
        ))
        .with_header("content-type", "application/json")
        .with_body(linked.to_string())
        .expect(1)
        .create_async()
        .await;

    let (task_raw, watch_raw) =
        bugs::link_bug_task_remote_raw(&client(&server), 42, &task_url, "debbugs", "#1150409")
            .await
            .unwrap();

    assert_eq!(task_raw.raw, linked);
    assert_eq!(watch_raw.raw, watch_body);
    assert_all(&[get_task, tracker_mock, list, post, get_watch, patch]).await;
}

#[tokio::test]
async fn link_bug_task_remote_raw_reused_watch_keeps_listed_entry() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}{TASK_PATH}");
    let watch_body = watch(
        &base,
        999,
        "debbugs",
        "1150409",
        "https://bugs.debian.org/1150409",
    );
    let get_task = mock_task(&mut server).await;
    let tracker_mock = mock_tracker_by_name(
        &mut server,
        &tracker(&base, "debbugs", "Debbugs", "https://bugs.debian.org/"),
    )
    .await;
    let list = mock_watches(&mut server, std::slice::from_ref(&watch_body)).await;
    let post = mock_never(&mut server, "POST", "/bugs/42").await;
    let patch = mock_json(&mut server, "PATCH", TASK_PATH, &task(&base, "New")).await;

    let (_, watch_raw) =
        bugs::link_bug_task_remote_raw(&client(&server), 42, &task_url, "debbugs", "1150409")
            .await
            .unwrap();

    assert_eq!(
        watch_raw.raw, watch_body,
        "the listed entry is passed through unchanged"
    );
    assert_all(&[get_task, tracker_mock, list, post, patch]).await;
}

#[tokio::test]
async fn link_bug_task_remote_raw_failures_after_creating_watch_are_partial() {
    for case in ["read-back", "patch", "patch-body"] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let task_url = format!("{base}{TASK_PATH}");
        let get_task = mock_task(&mut server).await;
        let tracker_mock = mock_tracker_by_name(
            &mut server,
            &tracker(&base, "debbugs", "Debbugs", "https://bugs.debian.org/"),
        )
        .await;
        let list = mock_watches(&mut server, &[]).await;
        let post = mock_created(
            &mut server,
            "/bugs/42",
            &format!("{base}/bugs/42/+watch/101"),
        )
        .await;
        let watch_body = watch(&base, 101, "debbugs", "1", "https://bugs.debian.org/1");
        let get_watch = match case {
            "read-back" => {
                server
                    .mock("GET", "/bugs/42/+watch/101")
                    .with_status(500)
                    .with_body("read failed")
                    .expect(1)
                    .create_async()
                    .await
            }
            _ => mock_json(&mut server, "GET", "/bugs/42/+watch/101", &watch_body).await,
        };
        let patch = match case {
            "read-back" => mock_never(&mut server, "PATCH", TASK_PATH).await,
            "patch" => {
                server
                    .mock("PATCH", TASK_PATH)
                    .with_status(500)
                    .with_body("task update failed")
                    .expect(1)
                    .create_async()
                    .await
            }
            _ => {
                server
                    .mock("PATCH", TASK_PATH)
                    .with_body("not json")
                    .expect(1)
                    .create_async()
                    .await
            }
        };

        let result =
            bugs::link_bug_task_remote_raw(&client(&server), 42, &task_url, "debbugs", "1").await;

        let fragments: &[&str] = match case {
            "read-back" => &["read failed", "Watch #101", "may remain"],
            "patch" => &["task update failed", "Watch #101", "may remain"],
            _ => &["Linking the bug task succeeded", "lpcli bug watches -b 42"],
        };
        assert_partial(result, fragments);
        assert_all(&[get_task, tracker_mock, list, post, get_watch, patch]).await;
    }
}

#[tokio::test]
async fn link_bug_task_remote_raw_reused_watch_patch_failure_is_not_partial() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}{TASK_PATH}");
    let get_task = mock_task(&mut server).await;
    let tracker_mock = mock_tracker_by_name(
        &mut server,
        &tracker(&base, "debbugs", "Debbugs", "https://bugs.debian.org/"),
    )
    .await;
    let list = mock_watches(
        &mut server,
        &[watch(
            &base,
            999,
            "debbugs",
            "1",
            "https://bugs.debian.org/1",
        )],
    )
    .await;
    let patch = server
        .mock("PATCH", TASK_PATH)
        .with_status(400)
        .with_body("nope")
        .expect(1)
        .create_async()
        .await;

    let result =
        bugs::link_bug_task_remote_raw(&client(&server), 42, &task_url, "debbugs", "1").await;

    assert!(
        matches!(result, Err(LpError::Api { status: 400, .. })),
        "{result:?}"
    );
    assert_all(&[get_task, tracker_mock, list, patch]).await;
}

#[tokio::test]
async fn link_bug_task_remote_raw_rejects_bad_input_as_usage_before_mutation() {
    // Foreign task URL: rejected before any request.
    let mut server = Server::new_async().await;
    let mut foreign = Server::new_async().await;
    let foreign_get = mock_never(&mut foreign, "GET", TASK_PATH).await;
    let post = mock_never(&mut server, "POST", "/bugs/42").await;
    let result = bugs::link_bug_task_remote_raw(
        &client(&server),
        42,
        &format!("{}{TASK_PATH}", foreign.url()),
        "debbugs",
        "1",
    )
    .await;
    assert_usage(result, "not on the configured Launchpad API origin");
    assert_all(&[foreign_get, post]).await;

    // Task on another bug.
    let mut server = Server::new_async().await;
    let base = server.url();
    let mut other_bug = task(&base, "New");
    other_bug["bug_link"] = json!(format!("{base}/bugs/999"));
    let get_task = mock_json(&mut server, "GET", TASK_PATH, &other_bug).await;
    let post = mock_never(&mut server, "POST", "/bugs/42").await;
    let result = bugs::link_bug_task_remote_raw(
        &client(&server),
        42,
        &format!("{base}{TASK_PATH}"),
        "x",
        "1",
    )
    .await;
    assert_usage(result, "not bug #42");
    assert_all(&[get_task, post]).await;

    // Non-numeric GitHub issue number on a GitHub tracker.
    let mut server = Server::new_async().await;
    let base = server.url();
    let get_task = mock_task(&mut server).await;
    let tracker_mock = mock_tracker_by_name(
        &mut server,
        &tracker(
            &base,
            "gh",
            bugs::BUG_TRACKER_TYPE_GITHUB,
            "https://github.com/o/r/issues",
        ),
    )
    .await;
    let list = mock_never(&mut server, "GET", "/bugs/42/bug_watches").await;
    let result = bugs::link_bug_task_remote_raw(
        &client(&server),
        42,
        &format!("{base}{TASK_PATH}"),
        "gh",
        "abc",
    )
    .await;
    assert_usage(result, "digits only");
    assert_all(&[get_task, tracker_mock, list]).await;
}

// ---------------------------------------------------------------------------
// Tracker registration
// ---------------------------------------------------------------------------

fn query_by_base_url(base_url: &str) -> Matcher {
    Matcher::AllOf(vec![
        Matcher::UrlEncoded("ws.op".into(), "queryByBaseURL".into()),
        Matcher::UrlEncoded("base_url".into(), base_url.into()),
    ])
}

async fn mock_query_by_base_url(server: &mut ServerGuard, base_url: &str, body: &str) -> Mock {
    server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(query_by_base_url(base_url))
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create_async()
        .await
}

#[tokio::test]
async fn ensure_bug_tracker_raw_reuses_existing_tracker_unchanged() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let base_url = "https://bugs.example.org/trac";
    let body = tracker(&base, "example", "Trac", base_url);
    let query = mock_query_by_base_url(&mut server, base_url, &body.to_string()).await;
    let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let params = EnsureBugTrackerParams {
        base_url,
        bug_tracker_type: "Trac",
        name: None,
        title: None,
        summary: None,
    };

    let outcome = bugs::ensure_bug_tracker_raw(&client(&server), &params)
        .await
        .unwrap();

    assert!(!outcome.created);
    assert_eq!(outcome.tracker.raw, body);
    assert_all(&[query, post]).await;
}

#[tokio::test]
async fn ensure_bug_tracker_raw_creates_and_classifies_failures() {
    let base_url = "https://bugs.example.org/trac";
    let params = EnsureBugTrackerParams {
        base_url,
        bug_tracker_type: "Trac",
        name: None,
        title: None,
        summary: None,
    };
    for case in ["ok", "read-back", "mismatch"] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let query = mock_query_by_base_url(&mut server, base_url, "null").await;
        let post = mock_created(
            &mut server,
            "/bugs/bugtrackers",
            &format!("{base}/bugs/bugtrackers/example"),
        )
        .await;
        let created = match case {
            "mismatch" => tracker(&base, "example", "Bugzilla", base_url),
            _ => tracker(&base, "example", "Trac", base_url),
        };
        let get = match case {
            "read-back" => {
                server
                    .mock("GET", "/bugs/bugtrackers/example")
                    .with_status(500)
                    .with_body("read failed")
                    .expect(1)
                    .create_async()
                    .await
            }
            _ => mock_json(&mut server, "GET", "/bugs/bugtrackers/example", &created).await,
        };

        let result = bugs::ensure_bug_tracker_raw(&client(&server), &params).await;

        match case {
            "ok" => {
                let outcome = result.unwrap();
                assert!(outcome.created);
                assert_eq!(outcome.tracker.raw, created);
            }
            "read-back" => assert_partial(result, &["read failed", "may have been created"]),
            _ => assert_partial(result, &["Bugzilla", "NOT modified"]),
        }
        assert_all(&[query, post, get]).await;
    }
}

#[tokio::test]
async fn ensure_bug_tracker_raw_rejects_invalid_requests_as_usage() {
    let mut server = Server::new_async().await;
    let never_get = mock_never(&mut server, "GET", "/bugs/bugtrackers").await;
    let never_post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    for (base_url, kind, fragment) in [
        ("  ", "Trac", "must not be empty"),
        (
            "git://github.com/o/r/issues",
            bugs::BUG_TRACKER_TYPE_GITHUB,
            "does not look like a GitHub",
        ),
    ] {
        let params = EnsureBugTrackerParams {
            base_url,
            bug_tracker_type: kind,
            name: None,
            title: None,
            summary: None,
        };
        let result = bugs::ensure_bug_tracker_raw(&client(&server), &params).await;
        assert_usage(result, fragment);
    }
    assert_all(&[never_get, never_post]).await;
}

#[tokio::test]
async fn link_github_bug_task_raw_after_registering_tracker_is_partial() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let task_url = format!("{base}{TASK_PATH}");
    let repo_base = "https://github.com/canonical/apport/issues";
    let get_task = mock_task(&mut server).await;
    let query = mock_query_by_base_url(&mut server, repo_base, "null").await;
    let post_tracker = mock_created(
        &mut server,
        "/bugs/bugtrackers",
        &format!("{base}/bugs/bugtrackers/auto-github-canonical-apport"),
    )
    .await;
    let get_tracker = mock_json(
        &mut server,
        "GET",
        "/bugs/bugtrackers/auto-github-canonical-apport",
        &tracker(
            &base,
            "auto-github-canonical-apport",
            bugs::BUG_TRACKER_TYPE_GITHUB,
            repo_base,
        ),
    )
    .await;
    let list = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(500)
        .with_body("watch list unavailable")
        .expect(1)
        .create_async()
        .await;
    let post_watch = mock_never(&mut server, "POST", "/bugs/42").await;
    let reference = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();

    let result = bugs::link_github_bug_task_raw(&client(&server), 42, &task_url, &reference).await;

    assert_partial(
        result,
        &[
            "Registered bug tracker",
            repo_base,
            "watch list unavailable",
        ],
    );
    assert_all(&[get_task, query, post_tracker, get_tracker, list, post_watch]).await;
}
