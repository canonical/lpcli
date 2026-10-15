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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url),
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

    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    post_watch_mock.assert_async().await;
    get_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_reuses_existing_watch() {
    let mut server = Server::new_async().await;

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

    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_watch_list_failure_without_mutations() {
    let mut server = Server::new_async().await;

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

    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_watch_creation_failure() {
    let mut server = Server::new_async().await;

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

    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}

#[tokio::test]
async fn link_bug_task_remote_propagates_task_patch_failure() {
    let mut server = Server::new_async().await;

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
            mockito::Matcher::UrlEncoded("bug_tracker".into(), tracker_url),
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

    assert!(matches!(result, Err(LpError::Api { status: 500, .. })));

    tracker_mock.assert_async().await;
    list_watches_mock.assert_async().await;
    add_watch_mock.assert_async().await;
    get_watch_mock.assert_async().await;
    patch_task_mock.assert_async().await;
}
