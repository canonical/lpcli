//! Raw JSON fidelity for Git, queue, webhook, snap, and access-token APIs.
//!
//! These tests check that `*_raw` functions return Launchpad's JSON
//! unmodified (including fields lpcli does not model and `null`s), that
//! collections keep Launchpad's shape, and that a failed read-back after a
//! successful mutation is reported as `LpError::Partial`.

use lpcli::{
    access_tokens,
    client::LaunchpadClient,
    error::{ErrorKind, LpError},
    git, queue, snaps, webhooks,
};
use mockito::{Matcher, Server, ServerGuard};
use serde_json::{Value, json};

fn client(server: &ServerGuard) -> LaunchpadClient {
    LaunchpadClient::new(None).with_base_url(server.url())
}

fn assert_partial(err: LpError, location: &str) {
    assert_eq!(err.kind(), ErrorKind::Partial, "{err:?}");
    match err {
        LpError::Partial(msg) => assert!(msg.contains(location), "{msg}"),
        other => panic!("expected Partial, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_repository_raw_preserves_launchpad_entry() {
    let mut server = Server::new_async().await;
    let body = json!({
        "self_link": "https://api.launchpad.net/devel/~jdoe/proj/+git/repo",
        "web_link": "https://code.launchpad.net/~jdoe/proj/+git/repo",
        "resource_type_link": "https://api.launchpad.net/devel/#git_repository",
        "http_etag": "\"abc-def\"",
        "name": "repo",
        "unique_name": "~jdoe/proj/+git/repo",
        "description": null,
        "repository_type": "Hosted",
        "information_type": "Public",
        "private": false,
        "date_created": "2024-01-02T03:04:05.123456+00:00",
        "unmodelled_field": {"nested": [1, 2, 3]}
    });
    let mock = server
        .mock("GET", "/~jdoe/proj/+git/repo")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let repo = git::get_git_repository_raw(&client(&server), "~jdoe/proj/+git/repo")
        .await
        .unwrap();

    assert_eq!(repo.raw, body);
    assert_eq!(repo.value.name.as_deref(), Some("repo"));
    mock.assert_async().await;
}

#[tokio::test]
async fn git_refs_raw_merges_pages_and_keeps_collection_metadata() {
    let mut server = Server::new_async().await;
    let next = format!(
        "{}/~jdoe/proj/+git/repo/refs?ws.start=1&ws.size=1",
        server.url()
    );
    let first_ref = json!({"path": "refs/heads/main", "commit_sha1": "aaa", "extra": null});
    let second_ref = json!({"path": "refs/tags/v1", "commit_sha1": "bbb", "extra": true});

    let page1 = server
        .mock("GET", "/~jdoe/proj/+git/repo/refs")
        .match_query(Matcher::Missing)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "total_size": 2,
                "start": 0,
                "next_collection_link": next,
                "entries": [first_ref],
                "resource_type_link": "https://api.launchpad.net/devel/#git_ref-page-resource"
            })
            .to_string(),
        )
        .create_async()
        .await;
    let page2 = server
        .mock("GET", "/~jdoe/proj/+git/repo/refs")
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "start": 1,
                "prev_collection_link": "ignored",
                "entries": [second_ref]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let refs = git::list_git_refs_raw(&client(&server), "/~jdoe/proj/+git/repo")
        .await
        .unwrap();

    assert_eq!(
        refs.raw,
        json!({"total_size": 2, "start": 0, "entries": [first_ref, second_ref]})
    );
    assert_eq!(refs.value.len(), 2);
    page1.assert_async().await;
    page2.assert_async().await;
}

#[tokio::test]
async fn merge_proposals_raw_passes_total_size_link_through() {
    let mut server = Server::new_async().await;
    let size_link = format!(
        "{}/~jdoe/proj/+git/repo?ws.op=getMergeProposals&ws.show=total_size",
        server.url()
    );
    let entry = json!({"queue_status": "Needs review", "self_link": "x/+merge/7", "votes": []});
    let mock = server
        .mock("GET", "/~jdoe/proj/+git/repo")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getMergeProposals".into()),
            Matcher::UrlEncoded("status".into(), "Needs review".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(
            json!({"total_size_link": size_link, "start": 0, "entries": [entry]}).to_string(),
        )
        .create_async()
        .await;

    let mps = git::list_merge_proposals_raw(
        &client(&server),
        "~jdoe/proj/+git/repo",
        Some("Needs review"),
    )
    .await
    .unwrap();

    assert_eq!(
        mps.raw,
        json!({"total_size_link": size_link, "start": 0, "entries": [entry]})
    );
    assert!(mps.raw.get("total_size").is_none());
    mock.assert_async().await;
}

#[tokio::test]
async fn inline_comments_raw_returns_launchpad_array() {
    let mut server = Server::new_async().await;
    let diff_link = format!(
        "{}/~jdoe/proj/+git/repo/+merge/7/+preview-diff/99",
        server.url()
    );
    let mp = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7")
        .match_query(Matcher::Missing)
        .with_header("content-type", "application/json")
        .with_body(json!({"preview_diff_link": diff_link}).to_string())
        .create_async()
        .await;
    let comments_body = json!([
        {
            "line_number": "12",
            "person": {"display_name": "J Doe", "name": "jdoe"},
            "text": "Nit: typo",
            "date": "2024-05-06T07:08:09+00:00",
            "unmodelled": null
        }
    ]);
    let comments = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getInlineComments".into()),
            Matcher::UrlEncoded("previewdiff_id".into(), "99".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(comments_body.to_string())
        .create_async()
        .await;
    let diff = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7/+preview-diff/99")
        .with_header("content-type", "application/json")
        .with_body(json!({"id": 99, "diff_text_link": null}).to_string())
        .create_async()
        .await;

    let (raw, map) =
        git::get_inline_comments_raw(&client(&server), "~jdoe/proj/+git/repo", 7, None)
            .await
            .unwrap();

    assert_eq!(raw.raw, comments_body);
    assert!(raw.raw.is_array());
    assert_eq!(raw.value.len(), 1);
    assert!(map.is_empty());
    mp.assert_async().await;
    comments.assert_async().await;
    diff.assert_async().await;
}

#[tokio::test]
async fn inline_comments_raw_without_preview_diff_is_empty_array() {
    let mut server = Server::new_async().await;
    let mp = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7")
        .with_header("content-type", "application/json")
        .with_body(json!({"preview_diff_link": null}).to_string())
        .create_async()
        .await;

    let (raw, map) =
        git::get_inline_comments_raw(&client(&server), "~jdoe/proj/+git/repo", 7, None)
            .await
            .unwrap();

    assert_eq!(raw.raw, json!([]));
    assert!(map.is_empty());
    mp.assert_async().await;
}

#[tokio::test]
async fn preview_diffs_raw_follows_collection_link() {
    let mut server = Server::new_async().await;
    let coll_link = format!(
        "{}/~jdoe/proj/+git/repo/+merge/7/preview_diffs",
        server.url()
    );
    let mp = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7")
        .with_header("content-type", "application/json")
        .with_body(json!({"preview_diffs_collection_link": coll_link}).to_string())
        .create_async()
        .await;
    let entry = json!({"id": 1, "title": "diff", "conflicts": null});
    let diffs = server
        .mock("GET", "/~jdoe/proj/+git/repo/+merge/7/preview_diffs")
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [entry]}).to_string())
        .create_async()
        .await;

    let raw = git::get_preview_diffs_raw(&client(&server), "~jdoe/proj/+git/repo", 7)
        .await
        .unwrap();

    assert_eq!(
        raw.raw,
        json!({"total_size": 1, "start": 0, "entries": [entry]})
    );
    mp.assert_async().await;
    diffs.assert_async().await;
}

// ---------------------------------------------------------------------------
// Upload queue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queue_search_page_raw_honours_limit_and_keeps_continuation() {
    let mut server = Server::new_async().await;
    let next = format!(
        "{}/ubuntu/resolute?ws.op=getPackageUploads&ws.size=1&ws.start=1",
        server.url()
    );
    let size_link = format!(
        "{}/ubuntu/resolute?ws.op=getPackageUploads&ws.show=total_size",
        server.url()
    );
    let entry = json!({
        "id": 42,
        "self_link": "https://api.launchpad.net/devel/ubuntu/resolute/+upload/42",
        "package_name": "mutter",
        "status": "New",
        "date_created": "2026-01-01T00:00:00.000001+00:00",
        "customfile_urls": [],
        "copy_source_archive_link": null
    });
    let page = json!({
        "total_size_link": size_link,
        "start": 0,
        "next_collection_link": next,
        "entries": [entry]
    });
    let first = server
        .mock("GET", "/ubuntu/resolute")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getPackageUploads".into()),
            Matcher::UrlEncoded("status".into(), "New".into()),
            Matcher::UrlEncoded("ws.size".into(), "1".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .expect(1)
        .create_async()
        .await;
    let second = server
        .mock("GET", "/ubuntu/resolute")
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .expect(0)
        .create_async()
        .await;

    let params = queue::QueueSearchParams {
        status: Some("New"),
        limit: Some(1),
        ..Default::default()
    };
    let raw = queue::get_package_uploads_page_raw(&client(&server), "ubuntu", "resolute", &params)
        .await
        .unwrap();

    assert_eq!(raw.raw, page);
    assert_eq!(raw.value.entries.len(), 1);
    assert_eq!(
        raw.value.next_collection_link.as_deref(),
        Some(next.as_str())
    );
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn queue_binary_properties_raw_preserves_unmodelled_fields() {
    let mut server = Server::new_async().await;
    let upload = format!("{}/ubuntu/resolute/+upload/42", server.url());
    let body = json!([
        {"name": "libfoo1", "version": "1.0", "architecture": "amd64", "is_new": true, "dbgsym": null}
    ]);
    let mock = server
        .mock("GET", "/ubuntu/resolute/+upload/42")
        .match_query(Matcher::UrlEncoded(
            "ws.op".into(),
            "getBinaryProperties".into(),
        ))
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let raw = queue::get_binary_properties_raw(&client(&server), &upload)
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    mock.assert_async().await;
}

#[tokio::test]
async fn queue_accept_raw_reads_back_upload() {
    let mut server = Server::new_async().await;
    let upload = format!("{}/ubuntu/resolute/+upload/42", server.url());
    let accept = server
        .mock("POST", "/ubuntu/resolute/+upload/42")
        .match_body(Matcher::UrlEncoded(
            "ws.op".into(),
            "acceptFromQueue".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("null")
        .create_async()
        .await;
    let body = json!({
        "id": 42,
        "self_link": upload,
        "status": "Accepted",
        "http_etag": "\"etag\"",
        "pocket": "Proposed"
    });
    let read = server
        .mock("GET", "/ubuntu/resolute/+upload/42")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let raw = queue::accept_from_queue_raw(&client(&server), &upload)
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    assert_eq!(raw.value.status.as_deref(), Some("Accepted"));
    accept.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn queue_reject_raw_read_back_failure_is_partial() {
    let mut server = Server::new_async().await;
    let upload = format!("{}/ubuntu/resolute/+upload/42", server.url());
    let reject = server
        .mock("POST", "/ubuntu/resolute/+upload/42")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "rejectFromQueue".into()),
            Matcher::UrlEncoded("comment".into(), "bad upload".into()),
        ]))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let read = server
        .mock("GET", "/ubuntu/resolute/+upload/42")
        .with_status(404)
        .with_body("gone")
        .create_async()
        .await;

    let err = queue::reject_from_queue_raw(&client(&server), &upload, Some("bad upload"))
        .await
        .unwrap_err();

    assert_partial(err, &upload);
    reject.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn queue_accept_raw_post_failure_is_not_partial() {
    let mut server = Server::new_async().await;
    let upload = format!("{}/ubuntu/resolute/+upload/42", server.url());
    let accept = server
        .mock("POST", "/ubuntu/resolute/+upload/42")
        .with_status(403)
        .with_body("forbidden")
        .create_async()
        .await;
    let read = server
        .mock("GET", "/ubuntu/resolute/+upload/42")
        .expect(0)
        .create_async()
        .await;

    let err = queue::accept_from_queue_raw(&client(&server), &upload)
        .await
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::PermissionDenied, "{err:?}");
    accept.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn queue_upload_logs_raw_keeps_collection_shape() {
    let mut server = Server::new_async().await;
    let logs = format!("{}/ubuntu/resolute/+upload/42/logs", server.url());
    let entry = json!({"old_status": "New", "new_status": "Rejected", "comment": null});
    let mock = server
        .mock("GET", "/ubuntu/resolute/+upload/42/logs")
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [entry]}).to_string())
        .create_async()
        .await;

    let raw = queue::get_upload_logs_raw(&client(&server), &logs)
        .await
        .unwrap();

    assert_eq!(
        raw.raw,
        json!({"total_size": 1, "start": 0, "entries": [entry]})
    );
    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_webhook_raw_returns_created_entry() {
    let mut server = Server::new_async().await;
    let location = format!("{}/launchpad/+webhook/5", server.url());
    let create = server
        .mock("POST", "/launchpad")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "newWebhook".into()),
            Matcher::UrlEncoded("delivery_url".into(), "https://example.com/hook".into()),
            Matcher::UrlEncoded("event_types".into(), "git:push:0.1".into()),
        ]))
        .with_status(201)
        .with_header("location", &location)
        .create_async()
        .await;
    let body = json!({
        "delivery_url": "https://example.com/hook",
        "active": true,
        "event_types": ["git:push:0.1"],
        "self_link": location,
        "web_link": null,
        "resource_type_link": "https://api.launchpad.net/devel/#webhook",
        "deliveries_collection_link": format!("{location}/deliveries")
    });
    let read = server
        .mock("GET", "/launchpad/+webhook/5")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let raw = webhooks::create_webhook_raw(
        &client(&server),
        "/launchpad",
        "https://example.com/hook",
        &["git:push:0.1"],
        true,
        None,
    )
    .await
    .unwrap();

    assert_eq!(raw.raw, body);
    create.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn create_webhook_raw_read_back_failure_is_partial() {
    let mut server = Server::new_async().await;
    let location = format!("{}/launchpad/+webhook/5", server.url());
    let create = server
        .mock("POST", "/launchpad")
        .with_status(201)
        .with_header("location", &location)
        .create_async()
        .await;
    let read = server
        .mock("GET", "/launchpad/+webhook/5")
        .with_status(404)
        .create_async()
        .await;

    let err = webhooks::create_webhook_raw(
        &client(&server),
        "launchpad",
        "https://example.com/hook",
        &["git:push:0.1"],
        true,
        Some("s3cret"),
    )
    .await
    .unwrap_err();

    let message = err.to_string();
    assert!(!message.contains("s3cret"), "{message}");
    assert_partial(err, &location);
    create.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn ping_webhook_raw_read_back_failure_is_partial() {
    let mut server = Server::new_async().await;
    let hook = format!("{}/launchpad/+webhook/5", server.url());
    let location = format!("{hook}/+delivery/9");
    let ping = server
        .mock("POST", "/launchpad/+webhook/5")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "ping".into()))
        .with_status(201)
        .with_header("location", &location)
        .create_async()
        .await;
    let read = server
        .mock("GET", "/launchpad/+webhook/5/+delivery/9")
        .with_status(500)
        .create_async()
        .await;

    let err = webhooks::ping_webhook_raw(&client(&server), &hook)
        .await
        .unwrap_err();

    assert_partial(err, &location);
    ping.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn webhook_lists_raw_keep_collection_shape() {
    let mut server = Server::new_async().await;
    let hook_entry = json!({"delivery_url": "https://example.com/hook", "secret": null});
    let delivery_entry =
        json!({"successful": true, "response_status_code": 200, "error_message": null});
    let hooks = server
        .mock("GET", "/launchpad/webhooks")
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [hook_entry]}).to_string())
        .create_async()
        .await;
    let deliveries = server
        .mock("GET", "/launchpad/+webhook/5/deliveries")
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [delivery_entry]}).to_string())
        .create_async()
        .await;
    let c = client(&server);

    let raw_hooks = webhooks::list_target_webhooks_raw(&c, "launchpad")
        .await
        .unwrap();
    let raw_deliveries =
        webhooks::list_deliveries_raw(&c, &format!("{}/launchpad/+webhook/5", server.url()))
            .await
            .unwrap();

    assert_eq!(
        raw_hooks.raw,
        json!({"total_size": 1, "start": 0, "entries": [hook_entry]})
    );
    assert_eq!(
        raw_deliveries.raw,
        json!({"total_size": 1, "start": 0, "entries": [delivery_entry]})
    );
    hooks.assert_async().await;
    deliveries.assert_async().await;
}

// ---------------------------------------------------------------------------
// Snaps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snap_raw_preserves_launchpad_entry() {
    let mut server = Server::new_async().await;
    let body = json!({
        "name": "my-snap",
        "store_name": null,
        "auto_build": false,
        "processors_collection_link": "https://api.launchpad.net/devel/~jdoe/+snap/my-snap/processors",
        "http_etag": "\"etag\""
    });
    let mock = server
        .mock("GET", "/~jdoe/+snap/my-snap")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let raw = snaps::get_snap_raw(&client(&server), "jdoe", "my-snap")
        .await
        .unwrap();

    assert_eq!(raw.raw, body);
    mock.assert_async().await;
}

#[tokio::test]
async fn request_snap_builds_raw_returns_created_entry() {
    let mut server = Server::new_async().await;
    let location = format!("{}/~jdoe/+snap/my-snap/+build-request/1", server.url());
    let post = server
        .mock("POST", "/~jdoe/+snap/my-snap")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "requestBuilds".into()))
        .with_status(201)
        .with_header("location", &location)
        .create_async()
        .await;
    let body = json!({
        "status": "Pending",
        "error_message": null,
        "builds_collection_link": format!("{location}/builds"),
        "self_link": location
    });
    let read = server
        .mock("GET", "/~jdoe/+snap/my-snap/+build-request/1")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let raw = snaps::request_snap_builds_raw(
        &client(&server),
        "jdoe",
        "my-snap",
        "https://api.launchpad.net/devel/ubuntu/+archive/primary",
        "Updates",
    )
    .await
    .unwrap();

    assert_eq!(raw.raw, body);
    post.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn request_snap_builds_raw_read_back_failure_is_partial() {
    let mut server = Server::new_async().await;
    let location = format!("{}/~jdoe/+snap/my-snap/+build-request/1", server.url());
    let post = server
        .mock("POST", "/~jdoe/+snap/my-snap")
        .with_status(201)
        .with_header("location", &location)
        .create_async()
        .await;
    let read = server
        .mock("GET", "/~jdoe/+snap/my-snap/+build-request/1")
        .with_status(404)
        .create_async()
        .await;

    let err = snaps::request_snap_builds_raw(
        &client(&server),
        "jdoe",
        "my-snap",
        "https://api.launchpad.net/devel/ubuntu/+archive/primary",
        "Updates",
    )
    .await
    .unwrap_err();

    assert_partial(err, &location);
    post.assert_async().await;
    read.assert_async().await;
}

#[tokio::test]
async fn snap_collections_raw_keep_collection_shape() {
    let mut server = Server::new_async().await;
    let snap_entry = json!({"name": "my-snap", "store_channels": ["stable"]});
    let build_entry =
        json!({"title": "build", "buildstate": "Needs building", "arch_tag": "amd64"});
    let by_store = server
        .mock("GET", "/+snaps")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "findByStoreName".into()),
            Matcher::UrlEncoded("store_name".into(), "my-snap".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [snap_entry]}).to_string())
        .create_async()
        .await;
    let by_owner = server
        .mock("GET", "/+snaps")
        .match_query(Matcher::UrlEncoded("ws.op".into(), "findByOwner".into()))
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [snap_entry]}).to_string())
        .create_async()
        .await;
    let pending = server
        .mock("GET", "/~jdoe/+snap/my-snap/pending_builds")
        .with_header("content-type", "application/json")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [build_entry]}).to_string())
        .create_async()
        .await;
    let c = client(&server);

    let expected_snaps = json!({"total_size": 1, "start": 0, "entries": [snap_entry]});
    assert_eq!(
        snaps::find_snaps_by_store_name_raw(&c, "my-snap")
            .await
            .unwrap()
            .raw,
        expected_snaps
    );
    assert_eq!(
        snaps::find_snaps_by_owner_raw(&c, "jdoe")
            .await
            .unwrap()
            .raw,
        expected_snaps
    );
    assert_eq!(
        snaps::get_snap_pending_builds_raw(&c, "jdoe", "my-snap")
            .await
            .unwrap()
            .raw,
        json!({"total_size": 1, "start": 0, "entries": [build_entry]})
    );
    by_store.assert_async().await;
    by_owner.assert_async().await;
    pending.assert_async().await;
}

// ---------------------------------------------------------------------------
// Access tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn access_token_lists_raw_keep_collection_shape() {
    let mut server = Server::new_async().await;
    let entry = json!({
        "description": "push",
        "scopes": ["repository:push"],
        "owner_link": "https://api.launchpad.net/devel/~jdoe",
        "date_expires": null
    });
    let page = json!({"total_size": 1, "start": 0, "entries": [entry]});
    let project = server
        .mock("GET", "/launchpad")
        .match_query(Matcher::UrlEncoded(
            "ws.op".into(),
            "getAccessTokens".into(),
        ))
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .create_async()
        .await;
    let repo = server
        .mock("GET", "/~jdoe/proj/+git/repo")
        .match_query(Matcher::UrlEncoded(
            "ws.op".into(),
            "getAccessTokens".into(),
        ))
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .create_async()
        .await;
    let c = client(&server);

    let project_raw = access_tokens::list_project_access_tokens_raw(&c, "launchpad")
        .await
        .unwrap();
    let repo_raw = access_tokens::list_git_access_tokens_raw(&c, "/~jdoe/proj/+git/repo")
        .await
        .unwrap();

    assert_eq!(project_raw.raw, page);
    assert_eq!(repo_raw.raw, page);
    assert_eq!(project_raw.value.len(), 1);
    project.assert_async().await;
    repo.assert_async().await;
}

#[tokio::test]
async fn issue_access_token_still_returns_plain_secret() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/launchpad")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "issueAccessToken".into()),
            Matcher::UrlEncoded("scopes".into(), "repository:push".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(Value::String("tok-secret".into()).to_string())
        .create_async()
        .await;

    let secret = access_tokens::issue_project_access_token(
        &client(&server),
        "launchpad",
        "push",
        &["repository:push"],
    )
    .await
    .unwrap();

    assert_eq!(secret, "tok-secret");
    mock.assert_async().await;
}
