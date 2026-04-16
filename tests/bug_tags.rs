//! Integration tests for bug tag management (`set_bug_tags`).
//!
//! These tests exercise `bugs::set_bug_tags` end-to-end against a local mock
//! server (mockito) and do **not** contact the real Launchpad API.
//!
//! Coverage:
//! * PATCH is sent to the correct URL (`/bugs/{id}`) with a JSON body whose
//!   `tags` field is the supplied slice.
//! * The returned `Bug` is deserialised and its `tags` field reflects the
//!   updated value.
//! * Setting tags to an empty slice sends `{"tags": []}` and returns a bug
//!   with an empty tag list.

use lpcli::{bugs, client::LaunchpadClient};
use mockito::Server;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal Launchpad Bug JSON body with the given tags.
fn bug_json(id: u64, tags: &[&str]) -> String {
    let tag_array = tags
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"{{
            "id": {id},
            "title": "Test bug",
            "description": null,
            "tags": [{tag_array}],
            "date_created": null,
            "date_last_updated": null,
            "self_link": "https://api.launchpad.net/devel/bugs/{id}",
            "web_link": "https://bugs.launchpad.net/bugs/{id}",
            "owner_link": null,
            "users_affected_count": null
        }}"#
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `set_bug_tags` PATCHes the correct URL and returns the updated bug.
#[tokio::test]
async fn set_bug_tags_patches_correct_url_and_returns_updated_bug() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/42")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(42, &["regression", "jammy"]))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let new_tags: Vec<String> = vec!["regression".into(), "jammy".into()];

    let bug = bugs::set_bug_tags(&client, 42, &new_tags)
        .await
        .expect("set_bug_tags should succeed");

    assert_eq!(bug.id, 42);
    assert_eq!(bug.tags, vec!["regression", "jammy"]);

    patch_mock.assert_async().await;
}

/// `set_bug_tags` with an empty slice sends the request and returns a bug
/// with no tags.
#[tokio::test]
async fn set_bug_tags_with_empty_slice_clears_tags() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/7")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(7, &[]))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let bug = bugs::set_bug_tags(&client, 7, &[])
        .await
        .expect("set_bug_tags with empty slice should succeed");

    assert_eq!(bug.id, 7);
    assert!(bug.tags.is_empty());

    patch_mock.assert_async().await;
}

/// `set_bug_tags` with a single tag sets only that tag.
#[tokio::test]
async fn set_bug_tags_single_tag() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/100")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(100, &["java"]))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let new_tags: Vec<String> = vec!["java".into()];

    let bug = bugs::set_bug_tags(&client, 100, &new_tags)
        .await
        .expect("set_bug_tags should succeed");

    assert_eq!(bug.tags, vec!["java"]);

    patch_mock.assert_async().await;
}

/// `set_bug_tags` surfaces API errors (e.g. 404) correctly.
#[tokio::test]
async fn set_bug_tags_surfaces_not_found_error() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/9999")
        .with_status(404)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let err = bugs::set_bug_tags(&client, 9999, &["sometag".to_string()])
        .await
        .expect_err("404 should be an error");

    assert!(
        matches!(err, lpcli::error::LpError::NotFound(_)),
        "expected NotFound, got: {err}"
    );

    patch_mock.assert_async().await;
}

/// `set_bug_tags` sends a JSON body that includes the `tags` array, confirmed
/// by checking that mockito receives the request at the expected path.
#[tokio::test]
async fn set_bug_tags_sends_json_body_with_tags_array() {
    let mut server = Server::new_async().await;

    // Match on the JSON body to verify the `tags` field is a proper JSON array.
    let patch_mock = server
        .mock("PATCH", "/bugs/55")
        .match_header("content-type", mockito::Matcher::Regex("application/json".into()))
        .match_body(mockito::Matcher::JsonString(
            r#"{"tags": ["cricket", "badminton"]}"#.into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(55, &["cricket", "badminton"]))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let new_tags: Vec<String> = vec!["cricket".into(), "badminton".into()];

    let bug = bugs::set_bug_tags(&client, 55, &new_tags)
        .await
        .expect("set_bug_tags should succeed");

    assert_eq!(bug.tags, vec!["cricket", "badminton"]);

    patch_mock.assert_async().await;
}
