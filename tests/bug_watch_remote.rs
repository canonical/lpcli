//! Integration tests for bug watch and remote bug linking (`link_bug_task_remote`, etc.).
//!
//! These tests exercise remote bug tracking end-to-end against a local mock
//! server (mockito) and do **not** contact the real Launchpad API.

use lpcli::{bugs, client::LaunchpadClient, error::LpError};
use mockito::Server;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bug_tracker_json(base_url: &str, name: &str, title: &str) -> String {
    format!(
        r#"{{
            "name": "{name}",
            "title": "{title}",
            "summary": "Bug tracker for {name}.",
            "base_url": "https://bugs.{name}.org/",
            "bug_tracker_type": "Debbugs",
            "self_link": "{base_url}/bugs/bugtrackers/{name}",
            "web_link": "https://bugs.launchpad.net/bugs/bugtrackers/{name}"
        }}"#
    )
}

fn bug_watch_json(
    base_url: &str,
    id: u64,
    bug_id: u64,
    tracker_name: &str,
    remote_bug: &str,
) -> String {
    format!(
        r#"{{
            "self_link": "{base_url}/bugs/{bug_id}/+watch/{id}",
            "bug_link": "{base_url}/bugs/{bug_id}",
            "bug_tracker_link": "{base_url}/bugs/bugtrackers/{tracker_name}",
            "remote_bug": "{remote_bug}",
            "title": "{tracker_name} Bug tracker #{remote_bug}",
            "url": "https://bugs.{tracker_name}.org/bug/{remote_bug}",
            "web_link": "https://bugs.launchpad.net/bugs/{bug_id}/+watch/{id}",
            "remote_status": "open",
            "remote_importance": "normal"
        }}"#
    )
}

fn bug_task_json(base_url: &str, task_path: &str, bug_id: u64, watch_link: Option<&str>) -> String {
    let watch_entry = match watch_link {
        Some(w) => format!(r#""bug_watch_link": "{w}""#),
        None => r#""bug_watch_link": null"#.to_string(),
    };
    format!(
        r#"{{
            "self_link": "{base_url}/{task_path}/+bug/{bug_id}",
            "bug_link": "{base_url}/bugs/{bug_id}",
            "title": "Bug #{bug_id} in {task_path}",
            "status": "New",
            "importance": "Undecided",
            "bug_target_display_name": "{task_path}",
            "target_link": "{base_url}/{task_path}",
            {watch_entry}
        }}"#
    )
}

/// Mock the bug task GET performed by `link_bug_task_remote`'s preflight.
async fn mock_task_get(server: &mut Server) -> mockito::Mock {
    let base = server.url();
    mock_json_get(
        server,
        "/debian/+source/curl/+bug/42",
        &bug_task_json(&base, "debian/+source/curl", 42, None),
    )
    .await
}

/// Mock a tracker fetched by name with an arbitrary type and base URL (or,
/// in the `_without_base_url` variant, no `base_url` field at all).
async fn mock_tracker_by_name(
    server: &mut Server,
    name: &str,
    tracker_type: &str,
    base_url: &str,
) -> mockito::Mock {
    let body = tracker_entry_json(&server.url(), name, tracker_type, Some(base_url));
    mock_get_tracker_by_name_body(server, name, &body).await
}

async fn mock_tracker_by_name_without_base_url(
    server: &mut Server,
    name: &str,
    tracker_type: &str,
) -> mockito::Mock {
    let body = tracker_entry_json(&server.url(), name, tracker_type, None);
    mock_get_tracker_by_name_body(server, name, &body).await
}

#[tokio::test]
async fn get_bug_tracker_by_name_returns_tracker() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let tracker = bugs::get_bug_tracker_by_name(&client, "debbugs")
        .await
        .expect("get_bug_tracker_by_name should succeed");

    assert_eq!(tracker.name.as_deref(), Some("debbugs"));
    assert_eq!(tracker.title.as_deref(), Some("Debian Bug tracker"));
    assert_eq!(tracker.bug_tracker_type.as_deref(), Some("Debbugs"));

    mock.assert_async().await;
}

#[tokio::test]
async fn get_bug_tracker_by_name_not_found_returns_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=unknown")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("null")
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let result = bugs::get_bug_tracker_by_name(&client, "unknown").await;

    match result {
        Err(LpError::NotFound(msg)) => {
            assert!(
                msg.contains("unknown"),
                "Expected message to mention tracker name: {msg}"
            );
        }
        other => panic!("Expected LpError::NotFound, got {other:?}"),
    }

    mock.assert_async().await;
}

#[tokio::test]
async fn get_bug_trackers_returns_registered_trackers() {
    let mut server = Server::new_async().await;
    let first_page_entries = (0..100)
        .map(|index| {
            bug_tracker_json(
                &server.url(),
                &format!("github-project-{index}"),
                "GitHub project",
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let first_page =
        format!(r#"{{"entries": [{first_page_entries}], "total_size": 101, "start": 0}}"#);
    let final_page = format!(
        r#"{{"entries": [{}], "total_size": 101, "start": 100}}"#,
        bug_tracker_json(&server.url(), "github-project-final", "GitHub project")
    );

    let first_page_mock = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::UrlEncoded("ws.size".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(first_page)
        .create_async()
        .await;
    let final_page_mock = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.size".into(), "100".into()),
            mockito::Matcher::UrlEncoded("memo".into(), "100".into()),
            mockito::Matcher::UrlEncoded("ws.start".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(final_page)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let trackers = bugs::get_bug_trackers(&client)
        .await
        .expect("get_bug_trackers should succeed");

    assert_eq!(trackers.len(), 101);
    assert_eq!(trackers[0].name.as_deref(), Some("github-project-0"));
    assert_eq!(trackers[0].title.as_deref(), Some("GitHub project"));
    assert_eq!(trackers[100].name.as_deref(), Some("github-project-final"));

    first_page_mock.assert_async().await;
    final_page_mock.assert_async().await;
}

#[tokio::test]
async fn search_bug_trackers_filters_across_pages_and_stops_at_limit() {
    let mut server = Server::new_async().await;
    let second_page_url = format!(
        "{}/bugs/bugtrackers?ws.size=100&memo=100&ws.start=100",
        server.url()
    );
    let first_page = format!(
        r#"{{"entries": [{}], "next_collection_link": "{}"}}"#,
        bug_tracker_json(&server.url(), "unrelated", "Other tracker"),
        second_page_url
    );
    let second_page = format!(
        r#"{{"entries": [{}, {}, {}], "next_collection_link": "{}/bugs/bugtrackers?ws.size=100&memo=200&ws.start=200"}}"#,
        bug_tracker_json(&server.url(), "github-one", "GitHub tracker"),
        bug_tracker_json(&server.url(), "github-two", "GitHub tracker"),
        bug_tracker_json(&server.url(), "github-three", "GitHub tracker"),
        server.url()
    );

    let first_page_mock = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::UrlEncoded("ws.size".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(first_page)
        .create_async()
        .await;
    let second_page_mock = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.size".into(), "100".into()),
            mockito::Matcher::UrlEncoded("memo".into(), "100".into()),
            mockito::Matcher::UrlEncoded("ws.start".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(second_page)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let trackers = bugs::search_bug_trackers(&client, Some("GITHUB"), Some(2))
        .await
        .expect("tracker search should succeed");

    assert_eq!(trackers.len(), 2);
    assert_eq!(trackers[0].name.as_deref(), Some("github-one"));
    assert_eq!(trackers[1].name.as_deref(), Some("github-two"));
    first_page_mock.assert_async().await;
    second_page_mock.assert_async().await;
}

#[tokio::test]
async fn search_bug_trackers_raw_keeps_launchpad_json_and_reports_completeness() {
    let mut server = Server::new_async().await;
    let mut first: serde_json::Value =
        serde_json::from_str(&bug_tracker_json(&server.url(), "github-one", "GitHub")).unwrap();
    // Fields lpcli does not model must survive untouched.
    first["http_etag"] = "\"etag-1\"".into();
    first["contact_details"] = serde_json::Value::Null;
    let second: serde_json::Value =
        serde_json::from_str(&bug_tracker_json(&server.url(), "github-two", "GitHub")).unwrap();
    let page = serde_json::json!({"total_size": 2, "start": 0, "entries": [first, second]});
    let mock = server
        .mock("GET", "/bugs/bugtrackers")
        .match_query(mockito::Matcher::UrlEncoded("ws.size".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .expect(3)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    // Every page scanned: complete, with raw entries in Launchpad order.
    let found = bugs::search_bug_trackers_raw(&client, Some("github"), None)
        .await
        .unwrap();
    assert!(found.complete);
    assert_eq!(found.trackers.len(), 2);
    assert_eq!(found.trackers[0].raw, first);
    assert_eq!(found.trackers[0].value.name.as_deref(), Some("github-one"));

    // The limit is reached on the last entry of the last page: still complete.
    let found = bugs::search_bug_trackers_raw(&client, None, Some(2))
        .await
        .unwrap();
    assert!(found.complete);

    // The limit stops the scan with entries left: the total is unknown.
    let found = bugs::search_bug_trackers_raw(&client, None, Some(1))
        .await
        .unwrap();
    assert!(!found.complete);
    assert_eq!(found.trackers.len(), 1);
    mock.assert_async().await;
}

#[tokio::test]
async fn add_bug_watch_creates_watch_and_follows_location() {
    let mut server = Server::new_async().await;

    let tracker_url = format!("{}/bugs/bugtrackers/debbugs", server.url());
    let watch_url = format!("{}/bugs/42/+watch/101", server.url());

    let post_mock = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "addWatch".into()),
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url.clone()),
            mockito::Matcher::UrlEncoded("remote_bug".into(), "1150409".into()),
        ]))
        .with_status(201)
        .with_header("Location", &watch_url)
        .create_async()
        .await;

    let get_mock = server
        .mock("GET", "/bugs/42/+watch/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_watch_json(&server.url(), 101, 42, "debbugs", "1150409"))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let watch = bugs::add_bug_watch(&client, 42, &tracker_url, "1150409")
        .await
        .expect("add_bug_watch should succeed");

    assert_eq!(watch.remote_bug.as_deref(), Some("1150409"));
    assert_eq!(watch.id(), Some("101"));

    post_mock.assert_async().await;
    get_mock.assert_async().await;
}

#[tokio::test]
async fn get_bug_watches_returns_collection() {
    let mut server = Server::new_async().await;

    let watch1 = bug_watch_json(&server.url(), 101, 42, "debbugs", "1150409");
    let body = format!(
        r#"{{
            "entries": [{watch1}],
            "total_size": 1,
            "start": 0
        }}"#
    );

    let mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let watches = bugs::get_bug_watches(&client, 42)
        .await
        .expect("get_bug_watches should succeed");

    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0].remote_bug.as_deref(), Some("1150409"));

    mock.assert_async().await;
}

#[tokio::test]
async fn set_bug_watch_patches_task() {
    let mut server = Server::new_async().await;

    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let watch_url = format!("{}/bugs/42/+watch/101", server.url());

    let patch_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": watch_url
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_task_json(
            &server.url(),
            "debian/+source/curl",
            42,
            Some(&watch_url),
        ))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task = bugs::set_bug_watch(&client, &task_url, &watch_url)
        .await
        .expect("set_bug_watch should succeed");

    assert_eq!(task.bug_watch_link.as_deref(), Some(watch_url.as_str()));
    assert_eq!(task.bug_watch_id(), Some("101"));

    patch_mock.assert_async().await;
}

#[tokio::test]
async fn unset_bug_watch_clears_link() {
    let mut server = Server::new_async().await;

    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());

    let patch_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": null
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_task_json(
            &server.url(),
            "debian/+source/curl",
            42,
            None,
        ))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task = bugs::unset_bug_watch(&client, &task_url)
        .await
        .expect("unset_bug_watch should succeed");

    assert!(task.bug_watch_link.is_none());

    patch_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_end_to_end_creates_watch_and_links_task() {
    let mut server = Server::new_async().await;

    let get_task_mock = mock_task_get(&mut server).await;

    let tracker_mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;

    let list_watches_mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"entries": [], "total_size": 0, "start": 0}"#)
        .create_async()
        .await;

    let tracker_url = format!("{}/bugs/bugtrackers/debbugs", server.url());
    let watch_url = format!("{}/bugs/42/+watch/101", server.url());

    let post_watch_mock = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "addWatch".into()),
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url.clone()),
            mockito::Matcher::UrlEncoded("remote_bug".into(), "1150409".into()),
        ]))
        .with_status(201)
        .with_header("Location", &watch_url)
        .create_async()
        .await;

    let get_watch_mock = server
        .mock("GET", "/bugs/42/+watch/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_watch_json(&server.url(), 101, 42, "debbugs", "1150409"))
        .create_async()
        .await;

    let patch_task_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": watch_url
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_task_json(
            &server.url(),
            "debian/+source/curl",
            42,
            Some(&watch_url),
        ))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());

    let (task, watch) = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409")
        .await
        .expect("link_bug_task_remote should succeed");

    assert_eq!(task.bug_watch_link.as_deref(), Some(watch_url.as_str()));
    assert_eq!(watch.remote_bug.as_deref(), Some("1150409"));
    assert_eq!(watch.id(), Some("101"));

    get_task_mock.assert_async().await;
    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    post_watch_mock.assert_async().await;
    get_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_reuses_existing_watch() {
    let mut server = Server::new_async().await;

    let get_task_mock = mock_task_get(&mut server).await;

    let tracker_mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;

    let existing_watch = bug_watch_json(&server.url(), 999, 42, "debbugs", "1150409");
    let list_watches_mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"entries": [{existing_watch}], "total_size": 1, "start": 0}}"#
        ))
        .create_async()
        .await;

    let watch_url = format!("{}/bugs/42/+watch/999", server.url());

    let patch_task_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": watch_url
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_task_json(
            &server.url(),
            "debian/+source/curl",
            42,
            Some(&watch_url),
        ))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());

    // Call link_bug_task_remote with leading '#' in remote_bug to verify normalization
    let (task, watch) = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "#1150409")
        .await
        .expect("link_bug_task_remote should succeed");

    assert_eq!(task.bug_watch_link.as_deref(), Some(watch_url.as_str()));
    assert_eq!(watch.id(), Some("999"));

    get_task_mock.assert_async().await;
    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_watch_list_failure_without_mutations() {
    let mut server = Server::new_async().await;

    let get_task_mock = mock_task_get(&mut server).await;

    let tracker_mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;

    let list_watches_mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(500)
        .with_body("watch list unavailable")
        .expect(1)
        .create_async()
        .await;

    let add_watch_mock = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "addWatch".into(),
        ))
        .expect(0)
        .create_async()
        .await;
    let patch_task_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .expect(0)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let result = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409").await;

    assert!(matches!(result, Err(LpError::Api { status: 500, .. })));

    get_task_mock.assert_async().await;
    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_watch_creation_failure() {
    let mut server = Server::new_async().await;

    let get_task_mock = mock_task_get(&mut server).await;

    let tracker_mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;
    let list_watches_mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"entries": [], "total_size": 0, "start": 0}"#)
        .create_async()
        .await;
    let add_watch_mock = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "addWatch".into(),
        ))
        .with_status(500)
        .with_body("watch creation failed")
        .expect(1)
        .create_async()
        .await;
    let patch_task_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .expect(0)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let result = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409").await;

    assert!(matches!(result, Err(LpError::Api { status: 500, .. })));

    get_task_mock.assert_async().await;
    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_task_patch_failure() {
    let mut server = Server::new_async().await;

    let get_task_mock = mock_task_get(&mut server).await;

    let tracker_mock = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;
    let list_watches_mock = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"entries": [], "total_size": 0, "start": 0}"#)
        .create_async()
        .await;

    let tracker_url = format!("{}/bugs/bugtrackers/debbugs", server.url());
    let watch_url = format!("{}/bugs/42/+watch/101", server.url());
    let add_watch_mock = server
        .mock("POST", "/bugs/42")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "addWatch".into()),
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url.clone()),
            mockito::Matcher::UrlEncoded("remote_bug".into(), "1150409".into()),
        ]))
        .with_status(201)
        .with_header("Location", &watch_url)
        .create_async()
        .await;
    let get_watch_mock = server
        .mock("GET", "/bugs/42/+watch/101")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_watch_json(&server.url(), 101, 42, "debbugs", "1150409"))
        .create_async()
        .await;
    let patch_task_mock = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": watch_url
        })))
        .with_status(500)
        .with_body("task update failed")
        .expect(1)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let result = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409").await;

    // The watch was just created, so the PATCH failure must be enriched with
    // the watch ID, its web-UI deletion URL, and retry guidance — while
    // preserving the original error text.
    match result {
        Err(LpError::Other(msg)) => {
            assert!(msg.contains("500"), "unexpected message: {msg}");
            assert!(
                msg.contains("task update failed"),
                "unexpected message: {msg}"
            );
            assert!(msg.contains("Watch #101"), "unexpected message: {msg}");
            assert!(
                msg.contains("bugs.launchpad.net/bugs/42/+watch/101"),
                "unexpected message: {msg}"
            );
            assert!(msg.contains("may remain"), "unexpected message: {msg}");
        }
        other => panic!("Expected LpError::Other, got {other:?}"),
    }

    get_task_mock.assert_async().await;
    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    get_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn add_bug_watch_read_back_failures_name_the_created_watch() {
    // The addWatch POST succeeds, so the watch exists; a failed or malformed
    // read-back must keep the underlying error and identify the watch for
    // web-UI cleanup instead of reporting a bare API/JSON error.
    for (status, body, fragment) in [
        (403, "forbidden", "403"),
        (500, "server exploded", "server exploded"),
        (200, "{not json", "JSON error"),
    ] {
        let mut server = Server::new_async().await;
        let watch_url = format!("{}/bugs/42/+watch/101", server.url());
        let add_watch = server
            .mock("POST", "/bugs/42")
            .with_status(201)
            .with_header("Location", &watch_url)
            .expect(1)
            .create_async()
            .await;
        let read_back = server
            .mock("GET", "/bugs/42/+watch/101")
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let tracker_url = format!("{}/bugs/bugtrackers/debbugs", server.url());
        match bugs::add_bug_watch(&client, 42, &tracker_url, "1150409").await {
            Err(LpError::Other(msg)) => {
                for expected in [
                    fragment,
                    watch_url.as_str(),
                    "Watch #101",
                    "may remain",
                    "bugs.launchpad.net/bugs/42/+watch/101",
                    "lpcli bug watches -b 42",
                ] {
                    assert!(msg.contains(expected), "missing '{expected}' in: {msg}");
                }
            }
            other => panic!("Expected cleanup guidance for {status}, got {other:?}"),
        }
        add_watch.assert_async().await;
        read_back.assert_async().await;
    }
}

#[tokio::test]
async fn link_bug_task_remote_watch_read_back_failure_does_not_patch_task() {
    // The linking workflow propagates the enriched read-back error and never
    // patches the task with an unconfirmed watch.
    let mut server = Server::new_async().await;
    let get_task = mock_task_get(&mut server).await;
    let tracker = server
        .mock("GET", "/bugs/bugtrackers?ws.op=getByName&name=debbugs")
        .with_body(bug_tracker_json(
            &server.url(),
            "debbugs",
            "Debian Bug tracker",
        ))
        .create_async()
        .await;
    let list_watches = server
        .mock("GET", "/bugs/42/bug_watches")
        .with_body(r#"{"entries": [], "total_size": 0, "start": 0}"#)
        .create_async()
        .await;
    let add_watch = server
        .mock("POST", "/bugs/42")
        .with_status(201)
        .with_header("Location", &format!("{}/bugs/42/+watch/101", server.url()))
        .expect(1)
        .create_async()
        .await;
    let read_back = server
        .mock("GET", "/bugs/42/+watch/101")
        .with_status(500)
        .with_body("read failed")
        .expect(1)
        .create_async()
        .await;
    let patch_task = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .expect(0)
        .create_async()
        .await;
    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    match bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409").await {
        Err(LpError::Other(msg)) => assert!(
            msg.contains("read failed") && msg.contains("Watch #101") && msg.contains("may remain"),
            "unexpected message: {msg}"
        ),
        other => panic!("Expected cleanup guidance, got {other:?}"),
    }
    for mock in [
        get_task,
        tracker,
        list_watches,
        add_watch,
        read_back,
        patch_task,
    ] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn add_bug_watch_foreign_location_is_not_fetched_with_credentials() {
    let mut api = Server::new_async().await;
    let mut foreign = Server::new_async().await;
    let add_watch = api
        .mock("POST", "/bugs/42")
        .with_status(201)
        .with_header("Location", &format!("{}/bugs/42/+watch/101", foreign.url()))
        .expect(1)
        .create_async()
        .await;
    let foreign_request = foreign
        .mock("GET", mockito::Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let credentials = lpcli::auth::Credentials::new("lpcli", "token", "secret");
    let client = LaunchpadClient::new(Some(credentials)).with_base_url(api.url());
    let tracker_url = format!("{}/bugs/bugtrackers/debbugs", api.url());
    match bugs::add_bug_watch(&client, 42, &tracker_url, "1150409").await {
        Err(LpError::Other(msg)) => {
            assert!(
                msg.contains("not on the configured API origin")
                    && msg.contains("may have been created")
                    && msg.contains("lpcli bug watches -b 42"),
                "unexpected message: {msg}"
            );
            assert!(!msg.contains("secret"), "credentials leaked into: {msg}");
        }
        other => panic!("Expected an untrusted-location error, got {other:?}"),
    }
    add_watch.assert_async().await;
    foreign_request.assert_async().await;
}

// ---------------------------------------------------------------------------
// Bug tracker registration (ensureBugTracker) and the GitHub shorthand
// ---------------------------------------------------------------------------

const BUG_ID: u64 = 42;
const REPO_BASE_URL: &str = "https://github.com/canonical/apport/issues";
const TRACKER_NAME: &str = "auto-github-canonical-apport";
const TRACKER_TYPE: &str = "GitHub Issues";
const TASK_PATH: &str = "/ubuntu/+source/apport/+bug/42";
const VALID_WATCH_URL: &str = "https://github.com/canonical/apport/issues/658";
const TRAP_WATCH_URL: &str = "https://github.com/detail?id=658";

fn client_for(server: &Server) -> LaunchpadClient {
    LaunchpadClient::new(None).with_base_url(server.url())
}

/// A tracker entry JSON with the given identity (`base_url` omitted when
/// `None`).
#[rustfmt::skip]
fn tracker_entry_json(base: &str, name: &str, tracker_type: &str, base_url: Option<&str>) -> String {
    let base_url_field = match base_url {
        Some(base_url) => format!(r#""base_url": "{base_url}","#),
        None => r#""base_url": null,"#.to_string(),
    };
    format!(r#"{{"self_link": "{base}/bugs/bugtrackers/{name}", "web_link": "https://bugs.launchpad.net/bugs/bugtrackers/{name}", "name": "{name}", "title": "{name} Bug tracker", "summary": null, {base_url_field} "bug_tracker_type": "{tracker_type}"}}"#)
}

/// A GitHub-flavoured watch entry: derived URL `url`, remote bug id 658.
#[rustfmt::skip]
fn github_watch_json(base: &str, id: u64, url: &str) -> String {
    format!(r#"{{"self_link": "{base}/bugs/{BUG_ID}/+watch/{id}", "bug_link": "{base}/bugs/{BUG_ID}", "bug_tracker_link": "{base}/bugs/bugtrackers/{TRACKER_NAME}", "remote_bug": "658", "title": "{TRACKER_NAME} #658", "url": "{url}", "web_link": "https://bugs.launchpad.net/bugs/{BUG_ID}/+watch/{id}"}}"#)
}

/// The apport (Ubuntu) task entry whose bug_link points at
/// `bug_link_bug_id` (normally 42) with an optional watch link. Always
/// carries a valid `target_link` (real tasks have targets; bug watches do
/// not, and the preflight now rejects target-less entries).
#[rustfmt::skip]
fn apport_task_json(base: &str, bug_link_bug_id: u64, watch_link: Option<&str>) -> String {
    let watch_entry = match watch_link {
        Some(w) => format!(r#""bug_watch_link": "{w}""#),
        None => r#""bug_watch_link": null"#.to_string(),
    };
    format!(
        r#"{{"self_link": "{base}{TASK_PATH}", "bug_link": "{base}/bugs/{bug_link_bug_id}", "title": "Bug #{BUG_ID} in apport (Ubuntu)", "status": "New", "importance": "Undecided", "bug_target_display_name": "apport (Ubuntu)", "target_link": "{base}/ubuntu/+source/apport", {watch_entry}}}"#
    )
}

/// Plain 200 JSON GET (mockito's default status; the client checks status
/// only and does not care about the MIME type).
async fn mock_json_get(server: &mut Server, path: &str, body: &str) -> mockito::Mock {
    server
        .mock("GET", path)
        .with_body(body)
        .create_async()
        .await
}

fn query_by_base_url_path(base_url: &str) -> String {
    format!(
        "/bugs/bugtrackers?ws.op=queryByBaseURL&base_url={}",
        lpcli::client::urlenc(base_url)
    )
}

fn get_by_name_path(name: &str) -> String {
    format!(
        "/bugs/bugtrackers?ws.op=getByName&name={}",
        lpcli::client::urlenc(name)
    )
}

/// Mock the `queryByBaseURL` preflight lookup for `base_url`.
async fn mock_query_by_base_url(
    server: &mut Server,
    base_url: &str,
    body: &str,
    expect_hits: usize,
) -> mockito::Mock {
    server
        .mock("GET", query_by_base_url_path(base_url).as_str())
        .with_body(body)
        .expect(expect_hits)
        .create_async()
        .await
}

/// Mock the `getByName` tracker lookup for `name` returning a raw body.
async fn mock_get_tracker_by_name_body(
    server: &mut Server,
    name: &str,
    body: &str,
) -> mockito::Mock {
    server
        .mock("GET", get_by_name_path(name).as_str())
        .with_body(body)
        .create_async()
        .await
}

/// A mock that asserts the described request is never issued.
async fn mock_never(server: &mut Server, method: &str, path: &str) -> mockito::Mock {
    server.mock(method, path).expect(0).create_async().await
}

/// [`bugs::EnsureBugTrackerParams`] with an optional name; title/summary
/// unset (the via-location test extends this with its metadata).
fn ensure_params<'a>(
    base_url: &'a str,
    bug_tracker_type: &'a str,
    name: Option<&'a str>,
) -> bugs::EnsureBugTrackerParams<'a> {
    bugs::EnsureBugTrackerParams {
        base_url,
        bug_tracker_type,
        name,
        title: None,
        summary: None,
    }
}

/// Mock the apport task eligibility-preflight GET (valid `target_link`).
async fn mock_apport_task_get(server: &mut Server) -> mockito::Mock {
    let base = server.url();
    mock_json_get(server, TASK_PATH, &apport_task_json(&base, BUG_ID, None)).await
}

/// Mock the bug-watches collection returning the single entry `entries`.
async fn mock_list_watches(server: &mut Server, entries: &str) -> mockito::Mock {
    let body = if entries.is_empty() {
        r#"{"entries": [], "total_size": 0, "start": 0}"#.to_string()
    } else {
        format!(r#"{{"entries": [{entries}], "total_size": 1, "start": 0}}"#)
    };
    mock_json_get(server, &format!("/bugs/{BUG_ID}/bug_watches"), &body).await
}

/// Factory ensureBugTracker POST answering 201 + Location for
/// `tracker_name`, plus the follow-up GET returning `tracker_body`.
async fn mock_ensure_tracker_created(
    server: &mut Server,
    tracker_name: &str,
    tracker_body: &str,
) -> (mockito::Mock, mockito::Mock) {
    let post = server
        .mock("POST", "/bugs/bugtrackers")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "ensureBugTracker".into(),
        ))
        .with_status(201)
        .with_header(
            "Location",
            &format!("{}/bugs/bugtrackers/{tracker_name}", server.url()),
        )
        .create_async()
        .await;
    let fetch = mock_json_get(
        server,
        &format!("/bugs/bugtrackers/{tracker_name}"),
        tracker_body,
    )
    .await;
    (post, fetch)
}

/// Factory addWatch POST (explicit form-body match) answering 201 +
/// Location for the canonical issue number 658.
async fn mock_add_watch_created(
    server: &mut Server,
    tracker_url: &str,
    watch_id: u64,
) -> mockito::Mock {
    let base = server.url();
    server
        .mock("POST", format!("/bugs/{BUG_ID}").as_str())
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "addWatch".into()),
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url.into()),
            mockito::Matcher::UrlEncoded("remote_bug".into(), "658".into()),
        ]))
        .with_status(201)
        .with_header(
            "Location",
            &format!("{base}/bugs/{BUG_ID}/+watch/{watch_id}"),
        )
        .create_async()
        .await
}

/// Mock the apport task PATCH that links `watch_api_url`.
async fn mock_patch_watch_link(server: &mut Server, watch_api_url: &str) -> mockito::Mock {
    let base = server.url();
    server
        .mock("PATCH", TASK_PATH)
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "bug_watch_link": watch_api_url
        })))
        .with_body(apport_task_json(&base, BUG_ID, Some(watch_api_url)))
        .create_async()
        .await
}

async fn assert_all(mocks: &[mockito::Mock]) {
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn link_bug_task_remote_propagates_task_patch_failure_for_reused_watch() {
    // An existing matching watch is reused, so a PATCH failure propagates
    // unchanged: nothing irreversible was created by this call.
    let mut server = Server::new_async().await;
    let base = server.url();
    let get_task = mock_task_get(&mut server).await;
    let tracker_mock = mock_get_tracker_by_name_body(&mut server, "debbugs", &bug_tracker_json(&base, "debbugs", "Debian Bug tracker")).await;
    let list_watches = mock_list_watches(&mut server, &bug_watch_json(&base, 999, 42, "debbugs", "1150409")).await;
    let patch_task = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .with_status(400)
        .with_body("task update failed")
        .expect(1)
        .create_async()
        .await;
    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let result = bugs::link_bug_task_remote(&client, 42, &task_url, "debbugs", "1150409").await;
    assert!(matches!(result, Err(LpError::Api { status: 400, .. })));
    assert_all(&[get_task, tracker_mock, list_watches, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_bug_task_remote_canonicalises_github_remote_bug_number() {
    // The watch must be created with the canonical number (658), not the
    // raw input (0658), so the derived URL is canonical; the explicit
    // tracker identity is used as given (never auto-switched).
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_url = format!("{base}/bugs/bugtrackers/{TRACKER_NAME}");
    let get_task = mock_task_get(&mut server).await;
    let tracker_mock = mock_tracker_by_name(&mut server, TRACKER_NAME, TRACKER_TYPE, REPO_BASE_URL).await;
    let list_watches = mock_list_watches(&mut server, "").await;
    let post_watch = mock_add_watch_created(&mut server, &tracker_url, 101).await;
    let get_watch = mock_json_get(&mut server, "/bugs/42/+watch/101", &github_watch_json(&base, 101, VALID_WATCH_URL)).await;
    let watch_api_url = format!("{base}/bugs/42/+watch/101");
    let patch_task = server
        .mock("PATCH", "/debian/+source/curl/+bug/42")
        .match_body(mockito::Matcher::Json(serde_json::json!({ "bug_watch_link": watch_api_url })))
        .with_body(bug_task_json(&base, "debian/+source/curl", 42, Some(&watch_api_url)))
        .create_async()
        .await;
    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
    let (_, watch) = bugs::link_bug_task_remote(&client, 42, &task_url, TRACKER_NAME, "0658")
        .await
        .expect("link_bug_task_remote should canonicalise the GitHub remote bug");
    assert_eq!(watch.remote_bug.as_deref(), Some("658"));
    assert_all(&[get_task, tracker_mock, list_watches, post_watch, get_watch, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_creates_tracker_via_location() {
    // ensureBugTracker answers 201 + Location and is followed to fetch the
    // tracker; explicit name plus optional title/summary are sent as given.
    let mut server = Server::new_async().await;
    let base = server.url();
    let base_url = "https://bugs.example.org/trac/proj";
    let preflight = mock_query_by_base_url(&mut server, base_url, "null", 1).await;
    let get_by_name = mock_get_tracker_by_name_body(&mut server, "my-tracker", "null").await;
    let post = server
        .mock("POST", "/bugs/bugtrackers")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "ensureBugTracker".into()),
            mockito::Matcher::UrlEncoded("base_url".into(), base_url.into()),
            mockito::Matcher::UrlEncoded("bug_tracker_type".into(), "Trac".into()),
            mockito::Matcher::UrlEncoded("name".into(), "my-tracker".into()),
            mockito::Matcher::UrlEncoded("title".into(), "Trac instance".into()),
            mockito::Matcher::UrlEncoded("summary".into(), "Self-hosted Trac".into()),
        ]))
        .with_status(201)
        .with_header("Location", &format!("{base}/bugs/bugtrackers/my-tracker"))
        .create_async()
        .await;
    let fetch = mock_json_get(
        &mut server,
        "/bugs/bugtrackers/my-tracker",
        &tracker_entry_json(&base, "my-tracker", "Trac", Some(base_url)),
    )
    .await;
    let params = bugs::EnsureBugTrackerParams {
        base_url,
        bug_tracker_type: "Trac",
        name: Some("my-tracker"),
        title: Some("Trac instance"),
        summary: Some("Self-hosted Trac"),
    };
    let outcome = bugs::ensure_bug_tracker(&client_for(&server), &params)
        .await
        .expect("ensure_bug_tracker should create the tracker");
    assert!(outcome.created);
    assert_eq!(outcome.tracker.name.as_deref(), Some("my-tracker"));
    assert_eq!(outcome.tracker.bug_tracker_type.as_deref(), Some("Trac"));
    assert_all(&[preflight, get_by_name, post, fetch]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_foreign_location_is_not_fetched_with_credentials() {
    // Regression test: ensureBugTracker's Location used to be fetched without
    // an origin check, so a classic client would sign a request to it.
    let mut api = Server::new_async().await;
    let mut foreign = Server::new_async().await;
    let base_url = "https://bugs.example.org/trac/proj";
    let preflight = mock_query_by_base_url(&mut api, base_url, "null", 1).await;
    let post = api
        .mock("POST", "/bugs/bugtrackers")
        .with_status(201)
        .with_header("Location", &format!("{}/bugs/bugtrackers/my-tracker", foreign.url()))
        .expect(1)
        .create_async()
        .await;
    let foreign_request = foreign.mock("GET", mockito::Matcher::Any).expect(0).create_async().await;
    let credentials = lpcli::auth::Credentials::new("lpcli", "token", "secret");
    let client = LaunchpadClient::new(Some(credentials)).with_base_url(api.url());
    match bugs::ensure_bug_tracker(&client, &ensure_params(base_url, "Trac", None)).await {
        Err(LpError::Other(msg)) => {
            assert!(
                msg.contains("not on the configured API origin")
                    && msg.contains("may have been created")
                    && msg.contains("remote-bug-trackers --search"),
                "unexpected message: {msg}"
            );
            assert!(!msg.contains("secret"), "credentials leaked into: {msg}");
        }
        other => panic!("Expected an untrusted-location error, got {other:?}"),
    }
    assert_all(&[preflight, post, foreign_request]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_read_back_failure_reports_possible_creation() {
    for (status, body, fragment) in [(500, "read failed", "read failed"), (200, "{not json", "JSON error")] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let base_url = "https://bugs.example.org/trac/proj";
        let preflight = mock_query_by_base_url(&mut server, base_url, "null", 1).await;
        let post = server
            .mock("POST", "/bugs/bugtrackers")
            .with_status(201)
            .with_header("Location", &format!("{base}/bugs/bugtrackers/my-tracker"))
            .expect(1)
            .create_async()
            .await;
        let read_back = server
            .mock("GET", "/bugs/bugtrackers/my-tracker")
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        match bugs::ensure_bug_tracker(&client_for(&server), &ensure_params(base_url, "Trac", None)).await {
            Err(LpError::Other(msg)) => {
                for expected in [fragment, "/bugs/bugtrackers/my-tracker", "may have been created", base_url] {
                    assert!(msg.contains(expected), "missing '{expected}' in: {msg}");
                }
            }
            other => panic!("Expected possible-creation guidance, got {other:?}"),
        }
        assert_all(&[preflight, post, read_back]).await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_reuses_existing_tracker_without_posting() {
    // The preflight finds a tracker for this base URL and is answered
    // without a mutation-capable POST; the *returned* entry is
    // authoritative, so the requested name is never even consulted.
    let mut server = Server::new_async().await;
    let base = server.url();
    let base_url = "https://bugs.example.org/trac/proj";
    let preflight = mock_query_by_base_url(&mut server, base_url, &tracker_entry_json(&base, "trac-main", "Trac", Some(base_url)), 1).await;
    let get_by_name = mock_never(&mut server, "GET", &get_by_name_path("my-tracker")).await;
    let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let outcome = bugs::ensure_bug_tracker(&client_for(&server), &ensure_params(base_url, "Trac", Some("my-tracker")))
        .await
        .expect("the preflight hit must be reused");
    assert!(!outcome.created);
    assert_eq!(outcome.tracker.name.as_deref(), Some("trac-main"));
    assert_all(&[preflight, get_by_name, post]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_explicit_name_collision_and_equivalence() {
    // An explicit name taken by a tracker with a different base URL fails
    // cleanly before any factory POST. Launchpad's base-URL permutations
    // keep the path case-sensitive, so https://example.com/Bugs is a
    // different key, while an http/https or trailing-slash difference is
    // the same tracker.
    for (returned_base, expect_collision) in [("https://example.com/Bugs", true), ("http://example.com/bugs/", false)] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let preflight = mock_query_by_base_url(&mut server, "https://example.com/bugs", "null", 1).await;
        let get_by_name = mock_get_tracker_by_name_body(
            &mut server,
            "my-tracker",
            &tracker_entry_json(&base, "my-tracker", "Bugzilla", Some(returned_base)),
        )
        .await;
        let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
        let result = bugs::ensure_bug_tracker(&client_for(&server), &ensure_params("https://example.com/bugs", "Bugzilla", Some("my-tracker"))).await;
        if expect_collision {
            match result {
                Err(LpError::Other(msg)) => assert!(
                    msg.contains("already used") && msg.contains("https://example.com/Bugs"),
                    "unexpected message: {msg}"
                ),
                other => panic!("Expected a name-collision error, got {other:?}"),
            }
        } else {
            let outcome = result.expect("scheme and trailing-slash variants are the same tracker");
            assert!(!outcome.created);
        }
        assert_all(&[preflight, get_by_name, post]).await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_rejects_mismatched_returned_tracker() {
    // ensureBugTracker keys on base_url and returns existing trackers
    // unchanged, so the library validates the returned entry once: a wrong
    // type or a materially different base URL must fail (no false
    // success/zero-creation claims), and nothing is re-posted.
    for (returned_type, returned_base, fragment) in [
        ("GitLab Issues", "https://bugs.example.org/trac/proj", "GitLab Issues"),
        ("Trac", "https://bugs.example.org/other", "does not match the requested"),
    ] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let base_url = "https://bugs.example.org/trac/proj";
        let preflight = mock_query_by_base_url(&mut server, base_url, "null", 1).await;
        let get_by_name = mock_get_tracker_by_name_body(&mut server, "my-tracker", "null").await;
        let post = server
            .mock("POST", "/bugs/bugtrackers")
            .with_status(201)
            .with_header("Location", &format!("{base}/bugs/bugtrackers/my-tracker"))
            .expect(1)
            .create_async()
            .await;
        let fetch = mock_json_get(&mut server, "/bugs/bugtrackers/my-tracker", &tracker_entry_json(&base, "my-tracker", returned_type, Some(returned_base))).await;
        match bugs::ensure_bug_tracker(&client_for(&server), &ensure_params(base_url, "Trac", Some("my-tracker"))).await {
            Err(LpError::Other(msg)) => assert!(
                msg.contains(fragment) && msg.contains("NOT modified"),
                "unexpected message: {msg}"
            ),
            other => panic!("Expected a mismatch error for {returned_type}, got {other:?}"),
        }
        assert_all(&[preflight, get_by_name, post, fetch]).await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_bug_tracker_propagates_api_errors_without_retry() {
    // A failing POST propagates unchanged; a 2xx reply without a Location
    // header is an error too (no fallback read, no re-POST); a GitHub base
    // URL that is not canonical fails as a pure parse before any API call.
    let mut server = Server::new_async().await;
    let preflight = mock_query_by_base_url(&mut server, "https://bugs.example.org/trac/proj", "null", 1).await;
    let post = server.mock("POST", "/bugs/bugtrackers").with_status(500).with_body("boom").expect(1).create_async().await;
    let result = bugs::ensure_bug_tracker(&client_for(&server), &ensure_params("https://bugs.example.org/trac/proj", "Trac", None)).await;
    assert!(matches!(result, Err(LpError::Api { status: 500, .. })));
    assert_all(&[preflight, post]).await;

    let mut server = Server::new_async().await;
    let base = server.url();
    let preflight = mock_query_by_base_url(&mut server, "https://bugs.example.org/trac/proj", "null", 1).await;
    let post = server
        .mock("POST", "/bugs/bugtrackers")
        .with_body(tracker_entry_json(&base, "my-tracker", "Trac", Some("https://bugs.example.org/trac/proj")))
        .expect(1)
        .create_async()
        .await;
    let result = bugs::ensure_bug_tracker(&client_for(&server), &ensure_params("https://bugs.example.org/trac/proj", "Trac", None)).await;
    match result {
        Err(error) => assert!(
            error.to_string().contains("Location") && error.to_string().contains("may have been created"),
            "missing-Location must propagate with recovery guidance: {error}"
        ),
        Ok(outcome) => panic!("a body-shaped reply must not be success: {outcome:?}"),
    }
    assert_all(&[preflight, post]).await;

    let mut server = Server::new_async().await;
    let bad_base = "git://github.com/canonical/apport/issues";
    let preflight = mock_never(&mut server, "GET", &query_by_base_url_path(bad_base)).await;
    let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let result = bugs::ensure_bug_tracker(&client_for(&server), &ensure_params(bad_base, bugs::BUG_TRACKER_TYPE_GITHUB, None)).await;
    match result {
        Err(LpError::Other(msg)) => assert!(
            msg.contains("does not look like a GitHub repository issues URL"),
            "unexpected message: {msg}"
        ),
        other => panic!("Expected a pure parse error, got {other:?}"),
    }
    assert_all(&[preflight, post]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_github_bug_tracker_registers_tracker_without_name() {
    // The tracker name is omitted, so Launchpad derives the
    // auto-github-<owner>-<repo> name and owns collision suffixing; only
    // ONE base-URL lookup happens (no derived-name precheck via getByName).
    let mut server = Server::new_async().await;
    let base = server.url();
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, "null", 1).await;
    let get_by_name = mock_never(&mut server, "GET", &get_by_name_path(TRACKER_NAME)).await;
    let post = server
        .mock("POST", "/bugs/bugtrackers")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "ensureBugTracker".into()),
            mockito::Matcher::UrlEncoded("base_url".into(), REPO_BASE_URL.into()),
            mockito::Matcher::UrlEncoded("bug_tracker_type".into(), TRACKER_TYPE.into()),
            mockito::Matcher::Regex("summary=".into()),
        ]))
        .with_status(201)
        .with_header("Location", &format!("{base}/bugs/bugtrackers/{TRACKER_NAME}"))
        .expect(1)
        .create_async()
        .await;
    let no_name_sent = server
        .mock("POST", "/bugs/bugtrackers")
        .match_body(mockito::Matcher::Regex("name=".into()))
        .expect(0)
        .create_async()
        .await;
    let fetch = mock_json_get(&mut server, &format!("/bugs/bugtrackers/{TRACKER_NAME}"), &tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL))).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    let outcome = bugs::ensure_github_bug_tracker(&client_for(&server), &r)
        .await
        .expect("ensure_github_bug_tracker should register the tracker");
    assert!(outcome.created);
    assert_eq!(outcome.tracker.name.as_deref(), Some(TRACKER_NAME));
    assert_eq!(outcome.tracker.base_url.as_deref(), Some(REPO_BASE_URL));
    assert_all(&[preflight, get_by_name, post, no_name_sent, fetch]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_github_bug_tracker_reuses_existing_correct_tracker() {
    // A tracker registered for this exact base URL is reused as-is (one
    // read, no POST, no name precheck); the mixed-case reference canonicalises.
    let mut server = Server::new_async().await;
    let base = server.url();
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, &tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL)), 1).await;
    let get_by_name = mock_never(&mut server, "GET", &get_by_name_path(TRACKER_NAME)).await;
    let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let r = bugs::parse_github_bug_ref("Canonical/Apport#658").unwrap();
    let outcome = bugs::ensure_github_bug_tracker(&client_for(&server), &r)
        .await
        .expect("the existing tracker must be reused");
    assert!(!outcome.created);
    assert_eq!(outcome.tracker.name.as_deref(), Some(TRACKER_NAME));
    assert_all(&[preflight, get_by_name, post]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn ensure_github_bug_tracker_rejects_mistyped_existing_tracker() {
    // The 'github-issues' trap: a tracker for this base URL exists but is
    // typed Google Code. The library's once-over validation rejects it
    // before any watch is created; nothing is modified.
    let mut server = Server::new_async().await;
    let base = server.url();
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, &tracker_entry_json(&base, "mistyped-apport", "Google Code", Some(REPO_BASE_URL)), 1).await;
    let post = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    match bugs::ensure_github_bug_tracker(&client_for(&server), &r).await {
        Err(LpError::Other(msg)) => assert!(
            msg.contains("Google Code") && msg.contains("NOT modified"),
            "unexpected message: {msg}"
        ),
        other => panic!("Expected a mistyped-tracker error, got {other:?}"),
    }
    assert_all(&[preflight, post]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_end_to_end_creates_tracker_and_watch() {
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_url = format!("{base}/bugs/bugtrackers/{TRACKER_NAME}");
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, "null", 1).await;
    let (post_tracker, fetch_tracker) = mock_ensure_tracker_created(&mut server, TRACKER_NAME, &tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL))).await;
    let list_watches = mock_list_watches(&mut server, "").await;
    let post_watch = mock_add_watch_created(&mut server, &tracker_url, 101).await;
    let fetch_watch = mock_json_get(&mut server, "/bugs/42/+watch/101", &github_watch_json(&base, 101, VALID_WATCH_URL)).await;
    let watch_api_url = format!("{base}/bugs/{BUG_ID}/+watch/101");
    let patch_task = mock_patch_watch_link(&mut server, &watch_api_url).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    let (task, watch) = bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r)
        .await
        .expect("link_github_bug_task should link the task");
    assert!(task.bug_watch_link.as_deref().unwrap().ends_with("/+watch/101"));
    assert_eq!(watch.id(), Some("101"));
    assert_eq!(watch.url.as_deref(), Some(VALID_WATCH_URL));
    assert_all(&[get_task, preflight, post_tracker, fetch_tracker, list_watches, post_watch, fetch_watch, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_reuses_tracker_watch_and_pull_urls() {
    // A tracker registered for the repository is reused (no POST) and the
    // existing watch for the issue number is reused rather than duplicated —
    // including when the reference was given as a pull-request URL.
    let mut server = Server::new_async().await;
    let base = server.url();
    let pull_watch_url = "https://github.com/canonical/apport/pull/658";
    let tracker_body = tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL));
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, &tracker_body, 1).await;
    let list_watches = mock_list_watches(&mut server, &github_watch_json(&base, 999, pull_watch_url)).await;
    let watch_api_url = format!("{base}/bugs/{BUG_ID}/+watch/999");
    let patch_task = mock_patch_watch_link(&mut server, &watch_api_url).await;
    let post_tracker = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
    let r = bugs::parse_github_bug_ref("https://github.com/Canonical/Apport/pull/658").unwrap();
    let (task, watch) = bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r)
        .await
        .expect("existing tracker and watch must be reused");
    assert!(task.bug_watch_link.as_deref().unwrap().ends_with("/+watch/999"));
    assert_eq!(watch.id(), Some("999"));
    assert_eq!(watch.url.as_deref(), Some(pull_watch_url));
    assert_all(&[get_task, preflight, list_watches, patch_task, post_tracker, post_watch]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_reuses_watch_registered_via_other_api_version() {
    // Canonical resource identity ignores the API version segment: a watch
    // registered via the 1.0 API is reused even though the tracker's
    // self_link has none.
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_body = tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL)).replace(
        &format!("{base}/bugs/bugtrackers/{TRACKER_NAME}"),
        &format!("{base}/devel/bugs/bugtrackers/{TRACKER_NAME}"),
    );
    let watch_body = github_watch_json(&base, 999, VALID_WATCH_URL).replace(
        &format!("{base}/bugs/bugtrackers/{TRACKER_NAME}"),
        &format!("{base}/1.0/bugs/bugtrackers/{TRACKER_NAME}"),
    );
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, &tracker_body, 1).await;
    let list_watches = mock_list_watches(&mut server, &watch_body).await;
    let watch_api_url = format!("{base}/bugs/{BUG_ID}/+watch/999");
    let patch_task = mock_patch_watch_link(&mut server, &watch_api_url).await;
    let post_tracker = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    let (_, watch) = bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r)
        .await
        .expect("a watch registered via another API version must be reused");
    assert_eq!(watch.id(), Some("999"));
    assert_eq!(watch.url.as_deref(), Some(VALID_WATCH_URL));
    assert_all(&[get_task, preflight, list_watches, patch_task, post_tracker, post_watch]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_fails_closed_on_ineligible_tasks() {
    // The task preflight runs before ANY mutation: a missing task, a task
    // belonging to another bug, a task with no bug_link, and a watch-shaped
    // JSON entry (same bug_link, but a bug watch never has a target_link)
    // all abort before the tracker lookup, watch creation, or task PATCH.
    // One public wrapper suffices (link_bug_task_remote shares the preflight).
    for (case, status, fragments) in [
        ("missing task", 404u16, vec![]),
        ("wrong bug", 200, vec!["999", "refusing"]),
        ("no bug_link", 200, vec!["no bug_link", "refusing"]),
        ("wrong entity", 200, vec!["no target_link", "NOT modified"]),
    ] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let body = match case {
            "missing task" => "not found".to_string(),
            "wrong bug" => apport_task_json(&base, 999, None),
            "no bug_link" => r#"{"self_link": "x", "title": "unexpected entry"}"#.to_string(),
            _ => format!(
                r#"{{"self_link": "{base}/bugs/{BUG_ID}/+watch/101", "bug_link": "{base}/bugs/{BUG_ID}", "remote_bug": "658"}}"#
            ),
        };
        let get_task = server.mock("GET", TASK_PATH).with_status(status as usize).with_body(&body).create_async().await;
        let preflight = mock_never(&mut server, "GET", &query_by_base_url_path(REPO_BASE_URL)).await;
        let post_tracker = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
        let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
        let patch_task = mock_never(&mut server, "PATCH", TASK_PATH).await;
        let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
        match bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r).await {
            Err(LpError::NotFound(_)) if fragments.is_empty() => {}
            Err(LpError::Other(msg)) => {
                for fragment in &fragments {
                    assert!(msg.contains(fragment), "unexpected message: {msg}");
                }
            }
            other => panic!("Expected an eligibility failure ({case}), got {other:?}"),
        }
        assert_all(&[get_task, preflight, post_tracker, post_watch, patch_task]).await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_rejects_reused_watch_with_invalid_url() {
    // A reused watch whose derived URL is junk (the Google Code trap shape)
    // blocks the task PATCH: the error carries the source validation error,
    // the watch ID and derived URL, and the truthful check-before-retry
    // guidance (the watch may remain on the bug).
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_body = tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL));
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, &tracker_body, 1).await;
    let list_watches = mock_list_watches(&mut server, &github_watch_json(&base, 999, TRAP_WATCH_URL)).await;
    let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
    let patch_task = mock_never(&mut server, "PATCH", TASK_PATH).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    match bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r).await {
        Err(LpError::Other(msg)) => assert!(
            msg.contains("Watch #999")
                && msg.contains("may remain")
                && msg.contains("bugs.launchpad.net/bugs/42/+watch/999"),
            "unexpected message: {msg}"
        ),
        other => panic!("Expected an invalid-watch-URL error, got {other:?}"),
    }
    assert_all(&[get_task, preflight, list_watches, post_watch, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_rejects_created_watch_with_invalid_url() {
    // A just-created watch with a junk derived URL is reported with the
    // validation error, the watch ID/derived URL and the check guidance;
    // the task is never patched.
    let mut server = Server::new_async().await;
    let base = server.url();
    let tracker_url = format!("{base}/bugs/bugtrackers/{TRACKER_NAME}");
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, "null", 1).await;
    let (post_tracker, fetch_tracker) = mock_ensure_tracker_created(&mut server, TRACKER_NAME, &tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL))).await;
    let list_watches = mock_list_watches(&mut server, "").await;
    let post_watch = mock_add_watch_created(&mut server, &tracker_url, 101).await;
    let fetch_watch = mock_json_get(&mut server, "/bugs/42/+watch/101", &github_watch_json(&base, 101, TRAP_WATCH_URL)).await;
    let patch_task = mock_never(&mut server, "PATCH", TASK_PATH).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    match bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r).await {
        Err(LpError::Other(msg)) => assert!(
            msg.contains("Watch #101")
                && msg.contains("may remain")
                && msg.contains("bugs.launchpad.net/bugs/42/+watch/101"),
            "unexpected message: {msg}"
        ),
        other => panic!("Expected an invalid-watch-URL error, got {other:?}"),
    }
    assert_all(&[get_task, preflight, post_tracker, fetch_tracker, list_watches, post_watch, fetch_watch, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_reports_cleanup_contexts_for_created_watch() {
    // When a just-created watch cannot be linked, the failure preserves the
    // source error and adds the truthful cleanup guidance — without
    // promising the link state: a failing PATCH, a watch response without a
    // self_link, and a PATCH whose 200 reply is not valid JSON (request
    // succeeded, response failed) all leave the link result unknown.
    for (case, fragments) in [
        ("patch 500", vec!["task update failed", "500", "Watch #101"]),
        ("no self_link", vec!["no self_link", "Watch #<unknown>"]),
        ("patch 200 invalid json", vec!["Watch #101"]),
    ] {
        let mut server = Server::new_async().await;
        let base = server.url();
        let tracker_url = format!("{base}/bugs/bugtrackers/{TRACKER_NAME}");
        let watch_body = match case {
            "no self_link" => format!(
                r#"{{"bug_link": "{base}/bugs/{BUG_ID}", "bug_tracker_link": "{base}/bugs/bugtrackers/{TRACKER_NAME}", "remote_bug": "658", "title": "{TRACKER_NAME} #658", "url": "{VALID_WATCH_URL}", "web_link": "https://bugs.launchpad.net/bugs/{BUG_ID}/+watch/101"}}"#
            ),
            _ => github_watch_json(&base, 101, VALID_WATCH_URL),
        };
        let get_task = mock_apport_task_get(&mut server).await;
        let preflight = mock_query_by_base_url(&mut server, REPO_BASE_URL, "null", 1).await;
        let (post_tracker, fetch_tracker) = mock_ensure_tracker_created(&mut server, TRACKER_NAME, &tracker_entry_json(&base, TRACKER_NAME, TRACKER_TYPE, Some(REPO_BASE_URL))).await;
        let list_watches = mock_list_watches(&mut server, "").await;
        let post_watch = mock_add_watch_created(&mut server, &tracker_url, 101).await;
        let fetch_watch = mock_json_get(&mut server, "/bugs/42/+watch/101", &watch_body).await;
        let patch_task = match case {
            "patch 500" => server.mock("PATCH", TASK_PATH).with_status(500).with_body("task update failed").expect(1).create_async().await,
            "patch 200 invalid json" => server.mock("PATCH", TASK_PATH).with_body("not json").expect(1).create_async().await,
            _ => mock_never(&mut server, "PATCH", TASK_PATH).await,
        };
        let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
        match bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r).await {
            Err(LpError::Other(msg)) => {
                assert!(msg.contains("may remain"), "unexpected message: {msg}");
                assert!(msg.contains("bugs.launchpad.net/bugs/42/+watch/101"), "unexpected message: {msg}");
                for fragment in &fragments {
                    assert!(msg.contains(fragment), "unexpected message: {msg}");
                }
            }
            other => panic!("Expected a cleanup-context error ({case}), got {other:?}"),
        }
        assert_all(&[get_task, preflight, post_tracker, fetch_tracker, list_watches, post_watch, fetch_watch, patch_task]).await;
    }
}

#[rustfmt::skip]
#[tokio::test]
async fn link_bug_task_remote_github_tracker_guards_fail_before_writes() {
    // A tracker typed GitHub Issues (or with a github.com base URL) is
    // validated strictly before any mutation: a wrong type (the
    // 'github-issues' Google Code trap), a repo-less/git-scheme/wrong-host/
    // lookalike base URL, and a missing base URL all fail before any watch
    // list, watch creation, or task PATCH. Hosts are compared exactly,
    // never by suffix. The explicit tracker identity is never auto-switched.
    let guard_message = "does not look like a GitHub repository issues URL";
    for (name, tracker_type, base_url, fragments) in [
        ("github-issues", "Google Code", Some("https://github.com/"), vec!["Google Code", "GitHub Issues"]),
        ("github-root", "GitHub Issues", Some("https://github.com/"), vec![guard_message]),
        ("git-scheme", "GitHub Issues", Some("git://github.com/canonical/apport/issues"), vec![guard_message]),
        ("other-host", "GitHub Issues", Some("https://example.com/canonical/apport/issues"), vec![guard_message]),
        ("lookalike", "GitHub Issues", Some("https://github.com.evil.com/canonical/apport/issues"), vec![guard_message]),
        ("baseless", "GitHub Issues", None, vec!["no base_url", "GitHub Issues"]),
    ] {
        let mut server = Server::new_async().await;
        let get_task = mock_task_get(&mut server).await;
        let tracker_mock = match base_url {
            Some(base) => mock_tracker_by_name(&mut server, name, tracker_type, base).await,
            None => mock_tracker_by_name_without_base_url(&mut server, name, tracker_type).await,
        };
        let list_watches = mock_never(&mut server, "GET", "/bugs/42/bug_watches").await;
        let add_watch = mock_never(&mut server, "POST", "/bugs/42").await;
        let patch_task = mock_never(&mut server, "PATCH", "/debian/+source/curl/+bug/42").await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let task_url = format!("{}/debian/+source/curl/+bug/42", server.url());
        let result = bugs::link_bug_task_remote(&client, 42, &task_url, name, "658").await;
        match result {
            Err(LpError::Other(msg)) => {
                for fragment in &fragments {
                    assert!(msg.contains(fragment), "unexpected message: {msg}");
                }
            }
            other => panic!("Expected a tracker guard failure for '{name}', got {other:?}"),
        }
        assert_all(&[get_task, tracker_mock, list_watches, add_watch, patch_task]).await;
    }
}

// ---------------------------------------------------------------------------
// Owner==repo references and remote-linking origin safety
// ---------------------------------------------------------------------------

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_links_owner_equals_repo_shorthand() {
    // owner and repo may be equal (babel/babel): the tracker base URL and
    // the derived watch URL both parse, so the shorthand links end to end —
    // the fix is not parser-only.
    let mut server = Server::new_async().await;
    let base = server.url();
    let repo_base_url = "https://github.com/babel/babel/issues";
    let tracker_url = format!("{base}/bugs/bugtrackers/auto-github-babel-babel");
    let watch_url = "https://github.com/babel/babel/issues/658";
    let get_task = mock_apport_task_get(&mut server).await;
    let preflight = mock_query_by_base_url(&mut server, repo_base_url, "null", 1).await;
    let (post_tracker, fetch_tracker) = mock_ensure_tracker_created(&mut server, "auto-github-babel-babel", &tracker_entry_json(&base, "auto-github-babel-babel", TRACKER_TYPE, Some(repo_base_url))).await;
    let list_watches = mock_list_watches(&mut server, "").await;
    let post_watch = mock_add_watch_created(&mut server, &tracker_url, 101).await;
    let fetch_watch = mock_json_get(&mut server, "/bugs/42/+watch/101", &github_watch_json(&base, 101, watch_url)).await;
    let watch_api_url = format!("{base}/bugs/{BUG_ID}/+watch/101");
    let patch_task = mock_patch_watch_link(&mut server, &watch_api_url).await;
    let r = bugs::parse_github_bug_ref("babel/babel#658").unwrap();
    let (task, watch) = bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{base}{TASK_PATH}"), &r)
        .await
        .expect("an owner==repo shorthand must link end to end");
    assert!(task.bug_watch_link.as_deref().unwrap().ends_with("/+watch/101"));
    assert_eq!(watch.url.as_deref(), Some(watch_url));
    assert_all(&[get_task, preflight, post_tracker, fetch_tracker, list_watches, post_watch, fetch_watch, patch_task]).await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_github_bug_task_rejects_foreign_task_url_before_any_request() {
    // Two localhost servers on one hostname with different ports: a task
    // URL on the foreign port is rejected by the origin guard before the
    // first request — even though its body would pass every later
    // bug_link/target_link check — and the configured API is never
    // contacted. The clients are anonymous, so zero requests is a hard
    // proof that no header could have leaked to the foreign origin.
    let mut server = Server::new_async().await;
    let mut foreign = Server::new_async().await;
    let spoofed_task = foreign
        .mock("GET", TASK_PATH)
        .with_body(apport_task_json(&server.url(), BUG_ID, None))
        .expect(0)
        .create_async()
        .await;
    let task_get = mock_never(&mut server, "GET", TASK_PATH).await;
    let tracker_lookup = mock_never(&mut server, "GET", &query_by_base_url_path(REPO_BASE_URL)).await;
    let post_tracker = mock_never(&mut server, "POST", "/bugs/bugtrackers").await;
    let list_watches = mock_never(&mut server, "GET", &format!("/bugs/{BUG_ID}/bug_watches")).await;
    let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
    let patch_task = mock_never(&mut server, "PATCH", TASK_PATH).await;
    let r = bugs::parse_github_bug_ref("canonical/apport#658").unwrap();
    let result = bugs::link_github_bug_task(&client_for(&server), BUG_ID, &format!("{}{TASK_PATH}", foreign.url()), &r).await;
    assert!(matches!(result, Err(LpError::Other(_))));
    assert_all(&[task_get, tracker_lookup, post_tracker, list_watches, post_watch, patch_task]).await;
    spoofed_task.assert_async().await;
}

#[rustfmt::skip]
#[tokio::test]
async fn link_bug_task_remote_rejects_foreign_tracker_url_without_fetching_it() {
    // An explicit http(s) tracker URL is fetched directly, so it must pass
    // the origin guard too: after a valid task preflight, a foreign tracker
    // URL (again a different port on the same hostname) is rejected
    // unfetched, and the configured API sees the preflight GET but no
    // mutation.
    let mut server = Server::new_async().await;
    let mut foreign = Server::new_async().await;
    let get_task = mock_apport_task_get(&mut server).await;
    let foreign_tracker = foreign
        .mock("GET", "/bugs/bugtrackers/debbugs")
        .expect(0)
        .create_async()
        .await;
    let tracker_on_api = mock_never(&mut server, "GET", "/bugs/bugtrackers/debbugs").await;
    let list_watches = mock_never(&mut server, "GET", &format!("/bugs/{BUG_ID}/bug_watches")).await;
    let post_watch = mock_never(&mut server, "POST", &format!("/bugs/{BUG_ID}")).await;
    let patch_task = mock_never(&mut server, "PATCH", TASK_PATH).await;
    let task_url = format!("{}{TASK_PATH}", server.url());
    let foreign_tracker_url = format!("{}/bugs/bugtrackers/debbugs", foreign.url());
    let result = bugs::link_bug_task_remote(&client_for(&server), BUG_ID, &task_url, &foreign_tracker_url, "1150409").await;
    assert!(matches!(result, Err(LpError::Other(_))));
    get_task.assert_async().await;
    assert_all(&[tracker_on_api, list_watches, post_watch, patch_task]).await;
    foreign_tracker.assert_async().await;
}
