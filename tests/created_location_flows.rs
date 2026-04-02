use lpcli::{
    bugs,
    client::LaunchpadClient,
    error::LpError,
    snaps,
    webhooks,
};
use mockito::Server;

#[tokio::test]
async fn create_bug_follows_location_and_returns_created_bug() {
    let mut server = Server::new_async().await;

    let bug_location = format!("{}/bugs/123", server.url());

    let create_bug_mock = server
        .mock("POST", "/bugs")
        .with_status(201)
        .with_header("location", &bug_location)
        .create_async()
        .await;

    let fetch_bug_mock = server
        .mock("GET", "/bugs/123")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "id": 123,
                "title": "Kernel panic on boot",
                "description": "Booting on amd64 crashes during init.",
                "tags": ["regression", "jammy"],
                "date_created": null,
                "date_last_updated": null,
                "self_link": "https://api.launchpad.net/devel/bugs/123",
                "web_link": "https://bugs.launchpad.net/bugs/123",
                "owner_link": null,
                "users_affected_count": 2
            }"#,
        )
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let bug = bugs::create_bug(
        &client,
        "launchpad",
        "Kernel panic on boot",
        "Booting on amd64 crashes during init.",
    )
    .await
    .expect("create_bug should follow Location and fetch the created bug");

    assert_eq!(bug.id, 123);
    assert_eq!(bug.title, "Kernel panic on boot");
    assert_eq!(bug.tags, vec!["regression", "jammy"]);

    create_bug_mock.assert_async().await;
    fetch_bug_mock.assert_async().await;
}

#[tokio::test]
async fn request_snap_builds_follows_location_and_returns_build_request() {
    let mut server = Server::new_async().await;

    let request_location = format!("{}/~jdoe/+snap/my-snap/+build-request/1", server.url());

    let request_build_mock = server
        .mock("POST", "/~jdoe/+snap/my-snap")
        .with_status(201)
        .with_header("location", &request_location)
        .create_async()
        .await;

    let fetch_request_mock = server
        .mock("GET", "/~jdoe/+snap/my-snap/+build-request/1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "status": "Pending",
                "error_message": null,
                "date_requested": null,
                "date_finished": null,
                "self_link": "https://api.launchpad.net/devel/~jdoe/+snap/my-snap/+build-request/1",
                "web_link": null
            }"#,
        )
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let req = snaps::request_snap_builds(
        &client,
        "jdoe",
        "my-snap",
        "https://api.launchpad.net/devel/ubuntu/+archive/primary",
        "Release",
    )
    .await
    .expect("request_snap_builds should follow Location and fetch the build request");

    assert_eq!(req.status.as_deref(), Some("Pending"));
    assert!(req.error_message.is_none());

    request_build_mock.assert_async().await;
    fetch_request_mock.assert_async().await;
}

#[tokio::test]
async fn webhook_create_and_ping_follow_location() {
    let mut server = Server::new_async().await;

    let webhook_location = format!("{}/launchpad/+webhook/1", server.url());
    let delivery_location = format!("{}/launchpad/+webhook/1/+delivery/1", server.url());
    let webhook_body = format!(
        r#"{{
                "delivery_url": "https://example.com/hook",
                "active": true,
                "event_types": ["git:push:0.1"],
                "target_link": "https://api.launchpad.net/devel/launchpad",
                "registrant_link": null,
                "date_created": null,
                "date_last_modified": null,
                "self_link": "{webhook_location}",
                "web_link": null
            }}"#
    );
    let delivery_body = format!(
        r#"{{
                "successful": true,
                "response_status_code": 200,
                "pending": false,
                "date_sent": null,
                "webhook_link": "{webhook_location}",
                "self_link": "{delivery_location}",
                "web_link": null
            }}"#
    );

    let create_webhook_mock = server
        .mock("POST", "/launchpad")
        .with_status(201)
        .with_header("location", &webhook_location)
        .create_async()
        .await;

    let fetch_webhook_mock = server
        .mock("GET", "/launchpad/+webhook/1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(webhook_body)
        .create_async()
        .await;

    let ping_webhook_mock = server
        .mock("POST", "/launchpad/+webhook/1")
        .with_status(201)
        .with_header("location", &delivery_location)
        .create_async()
        .await;

    let fetch_delivery_mock = server
        .mock("GET", "/launchpad/+webhook/1/+delivery/1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(delivery_body)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let webhook = webhooks::create_webhook(
        &client,
        "/launchpad",
        "https://example.com/hook",
        &["git:push:0.1"],
        true,
        None,
    )
    .await
    .expect("create_webhook should follow Location and fetch webhook");

    let delivery = webhooks::ping_webhook(
        &client,
        webhook
            .self_link
            .as_deref()
            .expect("mocked webhook should include self_link"),
    )
    .await
    .expect("ping_webhook should follow Location and fetch delivery");

    assert_eq!(delivery.successful, Some(true));
    assert_eq!(delivery.response_status_code, Some(200));

    create_webhook_mock.assert_async().await;
    fetch_webhook_mock.assert_async().await;
    ping_webhook_mock.assert_async().await;
    fetch_delivery_mock.assert_async().await;
}

#[tokio::test]
async fn created_location_requires_location_header() {
    let mut server = Server::new_async().await;

    let post_mock = server
        .mock("POST", "/bugs")
        .with_status(201)
        .create_async()
        .await;

    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let params = std::collections::HashMap::from([
        ("ws.op", "createBug"),
        ("title", "Missing Location header"),
        ("description", "This should fail"),
        ("target", "https://api.launchpad.net/devel/launchpad"),
    ]);

    let err = client
        .post_created_location("/bugs", &params)
        .await
        .expect_err("201 Created without Location must return an error");

    match err {
        LpError::Other(msg) => {
            assert!(msg.contains("Location"));
        }
        other => panic!("unexpected error variant: {other}"),
    }

    post_mock.assert_async().await;
}
