//! Integration tests for bug description editing (`set_bug_description`).
//!
//! These tests exercise `bugs::set_bug_description` end-to-end against a local
//! mock server (mockito) and do **not** contact the real Launchpad API.
//!
//! Coverage:
//! * PATCH is sent to the correct URL (`/bugs/{id}`) with a JSON body whose
//!   `description` field is the supplied text.
//! * The returned `Bug` is deserialised and its `description` field reflects
//!   the updated value.
//! * Multi-line descriptions are sent verbatim.
//! * API errors (e.g. 404) are surfaced correctly.

use lpcli::{bugs, client::LaunchpadClient};
use mockito::Server;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal Launchpad Bug JSON body with the given description.
fn bug_json(id: u64, description: &str) -> String {
    // Embed the description as a JSON string (escapes newlines and quotes).
    let description_json = serde_json::json!(description).to_string();
    format!(
        r#"{{
            "id": {id},
            "title": "Test bug",
            "description": {description_json},
            "tags": [],
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

/// `set_bug_description` PATCHes the correct URL and returns the updated bug.
#[tokio::test]
async fn set_bug_description_patches_correct_url_and_returns_updated_bug() {
    let mut server = Server::new_async().await;

    let new_description = "Crash when unlocking the session with gdm.";

    let patch_mock = server
        .mock("PATCH", "/bugs/42")
        .match_header(
            "content-type",
            mockito::Matcher::Regex("application/json".into()),
        )
        .match_body(mockito::Matcher::JsonString(
            r#"{"description": "Crash when unlocking the session with gdm."}"#.into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(42, new_description))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let bug = bugs::set_bug_description(&client, 42, new_description)
        .await
        .expect("set_bug_description should succeed");

    assert_eq!(bug.id, 42);
    assert_eq!(bug.description.as_deref(), Some(new_description));

    patch_mock.assert_async().await;
}

/// `set_bug_description` sends multi-line description text verbatim.
#[tokio::test]
async fn set_bug_description_sends_multiline_text_verbatim() {
    let mut server = Server::new_async().await;

    let new_description =
        "First line of the report.\n\nSteps to reproduce:\n1. Lock the session\n2. Unlock it";

    let patch_mock = server
        .mock("PATCH", "/bugs/7")
        .match_header(
            "content-type",
            mockito::Matcher::Regex("application/json".into()),
        )
        .match_body(mockito::Matcher::PartialJson(
            serde_json::json!({ "description": new_description }),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(bug_json(7, new_description))
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let bug = bugs::set_bug_description(&client, 7, new_description)
        .await
        .expect("set_bug_description should succeed");

    assert_eq!(bug.description.as_deref(), Some(new_description));

    patch_mock.assert_async().await;
}

/// `set_bug_description` surfaces a permission failure (HTTP 401) as
/// `LpError::Api`, matching the documented error contract: users without
/// `launchpad.Edit` on the bug (e.g. a comment-only locked bug) get 401,
/// not `LpError::Unauthorized`.
#[tokio::test]
async fn set_bug_description_surfaces_permission_error() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/123")
        .with_status(401)
        .with_header("content-type", "text/plain")
        .with_body("(<Bug at 0x7f00>, 'description', 'launchpad.Edit')")
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let err = bugs::set_bug_description(&client, 123, "no permission")
        .await
        .expect_err("401 should be an error");

    assert!(
        matches!(&err, lpcli::error::LpError::Api { status: 401, message }
            if message.contains("launchpad.Edit")),
        "expected Api(401) mentioning launchpad.Edit, got: {err}"
    );

    patch_mock.assert_async().await;
}

/// `set_bug_description` surfaces API errors (e.g. 404) correctly.
#[tokio::test]
async fn set_bug_description_surfaces_not_found_error() {
    let mut server = Server::new_async().await;

    let patch_mock = server
        .mock("PATCH", "/bugs/9999")
        .with_status(404)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let err = bugs::set_bug_description(&client, 9999, "no such bug")
        .await
        .expect_err("404 should be an error");

    assert!(
        matches!(err, lpcli::error::LpError::NotFound(_)),
        "expected NotFound, got: {err}"
    );

    patch_mock.assert_async().await;
}
