//! Integration tests for retry/back-off logic and non-2xx error classification.
//!
//! These tests exercise `LaunchpadClient` end-to-end against a local mock
//! server (mockito) and do **not** contact the real Launchpad API.
//!
//! Coverage:
//! * 429 → rate-limit (max_retries=0)
//! * 429 retried N times, then exhausted → `LpError::RateLimit`
//! * 429 Retry-After header value surfaced in the error
//! * 5xx retried N times, then exhausted → `LpError::Api`
//! * 401 / 403 / 404 / 400 returned immediately, never retried
//! * 200 with JSON body never triggers a retry

use std::collections::HashMap;

use lpcli::{client::LaunchpadClient, error::LpError};
use mockito::Server;

// ---------------------------------------------------------------------------
// Rate-limit (429) tests
// ---------------------------------------------------------------------------

/// With `max_retries=0` (the default) a single 429 response is immediately
/// surfaced as `LpError::RateLimit` without any retry attempt.
#[tokio::test]
async fn rate_limit_no_retry_returns_rate_limit_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/1")
        .with_status(429)
        .expect(1) // exactly one call – the default (no retry)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let err = client
        .get::<serde_json::Value>("/bugs/1")
        .await
        .expect_err("429 must be an error");

    assert!(
        matches!(err, LpError::RateLimit { .. }),
        "expected RateLimit, got: {err}"
    );

    mock.assert_async().await;
}

/// With `max_retries=2` on an endpoint that always returns 429 the client
/// should make exactly 3 calls (1 initial + 2 retries) before giving up.
#[tokio::test]
async fn rate_limit_retried_exhausted() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/1")
        .with_status(429)
        .expect(3) // initial + 2 retries
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(2)
        .with_retry_delay_ms(1); // keep the test fast

    let err = client
        .get::<serde_json::Value>("/bugs/1")
        .await
        .expect_err("exhausted 429s must be an error");

    assert!(
        matches!(err, LpError::RateLimit { .. }),
        "expected RateLimit, got: {err}"
    );

    mock.assert_async().await;
}

/// The `Retry-After` header value from the **final** 429 response (after all
/// retries are exhausted) is surfaced in the `retry_after_secs` field.
#[tokio::test]
async fn rate_limit_retry_after_header_is_captured() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/1")
        .with_status(429)
        .with_header("Retry-After", "42")
        .expect(2) // initial + 1 retry
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(1)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/bugs/1")
        .await
        .expect_err("exhausted 429s must be an error");

    match err {
        LpError::RateLimit { retry_after_secs: Some(secs) } => {
            assert_eq!(secs, 42, "Retry-After header value must be surfaced");
        }
        other => panic!("expected RateLimit with retry_after_secs=Some(42), got: {other}"),
    }

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// 5xx server-error retry tests
// ---------------------------------------------------------------------------

/// A 500 Internal Server Error is retried up to `max_retries` times and then
/// returned as `LpError::Api { status: 500 }`.
#[tokio::test]
async fn server_error_5xx_is_retried_and_returns_api_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/projects/ubuntu")
        .with_status(500)
        .with_body("internal server error")
        .expect(3) // initial + 2 retries
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(2)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/projects/ubuntu")
        .await
        .expect_err("5xx must be an error");

    match err {
        LpError::Api { status, .. } => {
            assert_eq!(status, 500, "status code must be preserved");
        }
        other => panic!("expected Api error, got: {other}"),
    }

    mock.assert_async().await;
}

/// A 503 Service Unavailable is treated the same as other 5xx errors.
#[tokio::test]
async fn server_error_503_is_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/2")
        .with_status(503)
        .expect(2) // initial + 1 retry
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(1)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/bugs/2")
        .await
        .expect_err("503 must be an error");

    assert!(
        matches!(err, LpError::Api { status: 503, .. }),
        "expected Api(503), got: {err}"
    );

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Non-retryable status codes
// ---------------------------------------------------------------------------

/// 401 Unauthorized is returned immediately as `LpError::Api` without any
/// retry.  Making one call is enough to surface auth failures quickly.
#[tokio::test]
async fn unauthorized_401_is_not_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/3")
        .with_status(401)
        .with_body("Unauthorized")
        .expect(1) // must be called exactly once
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(3)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/bugs/3")
        .await
        .expect_err("401 must be an error");

    assert!(
        matches!(err, LpError::Api { status: 401, .. }),
        "expected Api(401), got: {err}"
    );

    mock.assert_async().await;
}

/// 403 Forbidden is returned immediately without retry.
#[tokio::test]
async fn forbidden_403_is_not_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/4")
        .with_status(403)
        .with_body("Forbidden")
        .expect(1)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(3)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/bugs/4")
        .await
        .expect_err("403 must be an error");

    assert!(
        matches!(err, LpError::Api { status: 403, .. }),
        "expected Api(403), got: {err}"
    );

    mock.assert_async().await;
}

/// 404 Not Found is returned as `LpError::NotFound` without retry.
#[tokio::test]
async fn not_found_404_is_not_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/9999")
        .with_status(404)
        .expect(1)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(3)
        .with_retry_delay_ms(1);

    let err = client
        .get::<serde_json::Value>("/bugs/9999")
        .await
        .expect_err("404 must be an error");

    assert!(
        matches!(err, LpError::NotFound(..)),
        "expected NotFound, got: {err}"
    );

    mock.assert_async().await;
}

/// 400 Bad Request is returned as `LpError::Api` without retry.
#[tokio::test]
async fn bad_request_400_is_not_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/bugs")
        .with_status(400)
        .with_body("missing required field 'title'")
        .expect(1)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(3)
        .with_retry_delay_ms(1);

    let params = HashMap::from([
        ("ws.op", "createBug"),
        ("target", "https://api.launchpad.net/devel/launchpad"),
    ]);
    let err = client
        .post::<serde_json::Value>("/bugs", &params)
        .await
        .expect_err("400 must be an error");

    match err {
        LpError::Api { status, ref message } => {
            assert_eq!(status, 400);
            assert!(message.contains("field") || message.contains("400") || !message.is_empty());
        }
        other => panic!("expected Api(400), got: {other}"),
    }

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Successful response never triggers a retry
// ---------------------------------------------------------------------------

/// A 200 OK response is accepted immediately; the mock must be called exactly
/// once even when `max_retries` is set.
#[tokio::test]
async fn success_200_is_not_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("GET", "/bugs/10")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "id": 10,
                "title": "Sample bug",
                "description": "desc",
                "tags": [],
                "date_created": null,
                "date_last_updated": null,
                "self_link": "https://api.launchpad.net/devel/bugs/10",
                "web_link": "https://bugs.launchpad.net/bugs/10",
                "owner_link": null,
                "users_affected_count": 0
            }"#,
        )
        .expect(1) // exactly one call
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(3)
        .with_retry_delay_ms(1);

    let bug: lpcli::bugs::Bug = client
        .get("/bugs/10")
        .await
        .expect("200 OK must succeed");

    assert_eq!(bug.id, 10);

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// POST helpers propagate status codes correctly
// ---------------------------------------------------------------------------

/// A POST that returns 422 Unprocessable Entity is not retried and surfaces as
/// `LpError::Api`.
#[tokio::test]
async fn post_non_retryable_422_returns_api_error() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/bugs")
        .with_status(422)
        .with_body("Unprocessable Entity")
        .expect(1)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(2)
        .with_retry_delay_ms(1);

    let params = HashMap::from([("ws.op", "createBug"), ("title", "t"), ("target", "p")]);
    let err = client
        .post::<serde_json::Value>("/bugs", &params)
        .await
        .expect_err("422 must be an error");

    assert!(
        matches!(err, LpError::Api { status: 422, .. }),
        "expected Api(422), got: {err}"
    );

    mock.assert_async().await;
}

/// A POST that returns 500 is retried and eventually surfaces as
/// `LpError::Api { status: 500 }`.
#[tokio::test]
async fn post_5xx_is_retried() {
    let mut server = Server::new_async().await;

    let mock = server
        .mock("POST", "/bugs")
        .with_status(500)
        .with_body("server error")
        .expect(2) // initial + 1 retry
        .create_async()
        .await;

    let client = LaunchpadClient::new(None)
        .with_base_url(server.url())
        .with_max_retries(1)
        .with_retry_delay_ms(1);

    let params = HashMap::from([("ws.op", "createBug"), ("title", "t"), ("target", "p")]);
    let err = client
        .post::<serde_json::Value>("/bugs", &params)
        .await
        .expect_err("500 POST must be an error");

    assert!(
        matches!(err, LpError::Api { status: 500, .. }),
        "expected Api(500), got: {err}"
    );

    mock.assert_async().await;
}
