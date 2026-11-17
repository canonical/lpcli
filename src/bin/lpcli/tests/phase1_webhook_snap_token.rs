//! Phase 1 CLI tests for webhook, snap, and access-token commands.
//!
//! Every read and mutation runs against a mock Launchpad server. Reads are
//! snapshot-tested in each supported mode and their JSON must equal the
//! mocked Launchpad body exactly; mutations are checked in JSON and with
//! `--quiet`. Snapshots redact mock-server URLs, Launchpad timestamps, and
//! issued token secrets.

#[allow(unused_imports)]
use super::*;
use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use serde_json::{Value, json};

const MODES: [OutputMode; 4] = [
    OutputMode::Rich,
    OutputMode::Plain,
    OutputMode::Json,
    OutputMode::Tsv,
];

const LP: &str = "https://api.launchpad.net/devel";
const TIMESTAMP: &str = "2026-01-02T03:04:05.678901+00:00";
const TOKEN_SECRET: &str = "tok-0123456789abcdef";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse(args: &[&str]) -> Cli {
    let cli = Cli::try_parse_from(std::iter::once("lpcli").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("{args:?} must parse: {error}"));
    validate_cli(&cli).unwrap_or_else(|error| panic!("{args:?} must validate: {error}"));
    cli
}

fn webhook(args: &[&str]) -> WebhookCommand {
    match parse(&[&["webhook"], args].concat()).command {
        Command::Webhook(command) => command,
        other => panic!("not a webhook command: {other:?}"),
    }
}

fn snap(args: &[&str]) -> SnapCommand {
    match parse(&[&["snap"], args].concat()).command {
        Command::Snap(command) => command,
        other => panic!("not a snap command: {other:?}"),
    }
}

fn access_token(args: &[&str]) -> AccessTokenCommand {
    match parse(&[&["access-token"], args].concat()).command {
        Command::AccessToken(command) => command,
        other => panic!("not an access-token command: {other:?}"),
    }
}

fn render(result: &CommandResult, mode: OutputMode, quiet: bool) -> String {
    result.render(&Output::fixed(mode, false, 100), quiet)
}

fn redact_text(text: &str, base: &str) -> String {
    text.replace(base, "[server url]")
        .replace(TOKEN_SECRET, "[secret]")
}

/// Redact mock-server URLs, `date_*` timestamps, and token secrets.
fn redact_json(value: &Value, base: &str) -> Value {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                let value = if key.starts_with("date_") && value.is_string() {
                    "[timestamp]".into()
                } else {
                    redact_json(value, base)
                };
                (key.clone(), value)
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
        Value::Array(values) => values.iter().map(|v| redact_json(v, base)).collect(),
        Value::String(text) => redact_text(text, base).into(),
        value => value.clone(),
    }
}

/// Check one mode's invariants and snapshot it, returning the rendering.
fn snapshot_mode(
    name: &str,
    result: &CommandResult,
    mode: OutputMode,
    expected_json: &Value,
    records: Option<usize>,
    base: &str,
) -> String {
    let rendered = render(result, mode, false);
    match mode {
        OutputMode::Rich => assert!(!rendered.contains('\u{1b}'), "{rendered}"),
        OutputMode::Plain => assert_plain_invariants(&rendered),
        OutputMode::Tsv => {
            assert_tsv_invariants(&rendered, records.expect("tsv needs a record count"));
        }
        OutputMode::Json => {
            let parsed: Value = serde_json::from_str(&rendered).expect("json output must parse");
            assert_eq!(&parsed, expected_json, "JSON must match exactly");
            insta::assert_json_snapshot!(format!("{name}_json"), redact_json(&parsed, base));
            return rendered;
        }
    }
    insta::assert_snapshot!(
        format!("{name}_{}", mode.name()),
        redact_text(&rendered, base)
    );
    rendered
}

/// A read: JSON is the Launchpad body unchanged; lists (`records` is
/// `Some`) are also snapshotted in `tsv`.
fn assert_read(
    name: &str,
    result: &CommandResult,
    body: &Value,
    records: Option<usize>,
    base: &str,
) {
    let CommandResult::Read(report) = result else {
        panic!("{name} must be a read: {result:?}");
    };
    assert_eq!(
        &report.json, body,
        "{name}: JSON must be the Launchpad body"
    );
    for mode in MODES {
        if mode == OutputMode::Tsv && records.is_none() {
            continue;
        }
        snapshot_mode(name, result, mode, body, records, base);
    }
}

/// A mutation: snapshot `rich`, `plain`, and `json`, and check `--quiet`.
fn assert_mutation(
    name: &str,
    result: &CommandResult,
    json: &Value,
    quiet: Option<&str>,
    base: &str,
) {
    assert!(
        matches!(result, CommandResult::Mutation { .. }),
        "{name} must be a mutation: {result:?}"
    );
    for mode in [OutputMode::Rich, OutputMode::Plain, OutputMode::Json] {
        snapshot_mode(name, result, mode, json, None, base);
    }
    let expected = quiet.map(|line| format!("{line}\n")).unwrap_or_default();
    for mode in MODES {
        assert_eq!(
            render(result, mode, true),
            expected,
            "{name} --quiet in {mode:?}"
        );
    }
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn webhook_json(base: &str, id: u32, active: bool, events: Value) -> Value {
    json!({
        "active": active,
        "date_created": TIMESTAMP,
        "date_last_modified": TIMESTAMP,
        "delivery_url": format!("https://ci.example.com/hooks/{id}"),
        "event_types": events,
        "http_etag": "\"etag-webhook\"",
        "registrant_link": format!("{base}/~jdoe"),
        "resource_type_link": format!("{base}/#webhook"),
        "self_link": format!("{base}/launchpad/+webhook/{id}"),
        "target_link": format!("{base}/launchpad"),
        "web_link": format!("https://launchpad.net/launchpad/+webhook/{id}"),
    })
}

fn delivery_json(base: &str, id: u32, successful: Value, status: Value, pending: bool) -> Value {
    json!({
        "date_created": TIMESTAMP,
        "date_scheduled": null,
        "date_sent": TIMESTAMP,
        "error_message": null,
        "event_type": "ping",
        "http_etag": "\"etag-delivery\"",
        "payload": {"ping": true},
        "pending": pending,
        "resource_type_link": format!("{base}/#webhook_delivery"),
        "response_status_code": status,
        "self_link": format!("{base}/launchpad/+webhook/1/+delivery/{id}"),
        "successful": successful,
        "web_link": format!("https://launchpad.net/launchpad/+webhook/1/+delivery/{id}"),
        "webhook_link": format!("{base}/launchpad/+webhook/1"),
    })
}

fn snap_json(base: &str, name: &str, store_name: Value, private: bool) -> Value {
    json!({
        "auto_build": false,
        "date_created": TIMESTAMP,
        "description": "A snap built from the main branch.",
        "git_path": "refs/heads/main",
        "git_repository_link": format!("{base}/~jdoe/{name}/+git/{name}"),
        "git_repository_url": null,
        "http_etag": "\"etag-snap\"",
        "name": name,
        "owner_link": format!("{base}/~jdoe"),
        "private": private,
        "resource_type_link": format!("{base}/#snap"),
        "self_link": format!("{base}/~jdoe/+snap/{name}"),
        "store_name": store_name,
        "store_upload": true,
        "web_link": format!("https://launchpad.net/~jdoe/+snap/{name}"),
    })
}

fn snap_build_json(base: &str, id: u32, arch: &str, score: i64) -> Value {
    json!({
        "arch_tag": arch,
        "buildstate": "Needs building",
        "date_started": null,
        "distro_arch_series_link": format!("{base}/ubuntu/noble/{arch}"),
        "http_etag": "\"etag-build\"",
        "pocket": "Updates",
        "requester_link": format!("{base}/~jdoe"),
        "resource_type_link": format!("{base}/#snap_build"),
        "score": score,
        "self_link": format!("{base}/~jdoe/+snap/my-snap/+build/{id}"),
        "store_upload_status": "Unscheduled",
        "title": format!("{arch} build of my-snap snap package in ubuntu noble-updates"),
        "upload_log_url": null,
        "web_link": format!("https://launchpad.net/~jdoe/+snap/my-snap/+build/{id}"),
    })
}

fn access_token_json(base: &str, target: &str, id: u32, last_used: Value) -> Value {
    json!({
        "date_created": TIMESTAMP,
        "date_expires": null,
        "date_last_used": last_used,
        "description": format!("CI token {id}"),
        "http_etag": "\"etag-token\"",
        "is_expired": false,
        "owner_link": format!("{base}/~jdoe"),
        "resource_type_link": format!("{base}/#access_token"),
        "revoked_by_link": null,
        "scopes": ["repository:build_status", "repository:push"],
        "self_link": format!("{base}/{target}/+access-token/{id}"),
        "target_link": format!("{base}/{target}"),
        "web_link": null,
    })
}

// ---------------------------------------------------------------------------
// Structured output support and option validation
// ---------------------------------------------------------------------------

const WEBHOOK_URL: &str = "https://api.launchpad.net/devel/launchpad/+webhook/1";
const TOKEN_URL: &str = "https://api.launchpad.net/devel/launchpad/+access-token/1";

fn read_commands() -> Vec<Vec<&'static str>> {
    vec![
        vec!["webhook", "list", "--target", "launchpad"],
        vec!["webhook", "deliveries", "--webhook-url", WEBHOOK_URL],
        vec!["snap", "show", "--owner", "jdoe", "--name", "my-snap"],
        vec!["snap", "find", "--owner", "jdoe"],
        vec!["snap", "builds", "--owner", "jdoe", "--name", "my-snap"],
        vec!["access-token", "list", "--project", "launchpad"],
        vec![
            "access-token",
            "list-git",
            "--repo",
            "~jdoe/launchpad/+git/repo",
        ],
    ]
}

fn mutation_commands() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            "webhook",
            "create",
            "--target",
            "launchpad",
            "--delivery-url",
            "https://ci.example.com/hooks/3",
            "--event-types",
            "git:push:0.1",
        ],
        vec!["webhook", "delete", "--webhook-url", WEBHOOK_URL],
        vec!["webhook", "ping", "--webhook-url", WEBHOOK_URL],
        vec![
            "snap",
            "request-builds",
            "--owner",
            "jdoe",
            "--name",
            "my-snap",
        ],
        vec![
            "access-token",
            "issue",
            "--project",
            "launchpad",
            "--description",
            "CI",
            "--scopes",
            "repository:push",
        ],
        vec![
            "access-token",
            "issue-git",
            "--repo",
            "~jdoe/launchpad/+git/repo",
            "--description",
            "CI",
            "--scopes",
            "repository:push",
        ],
        vec!["access-token", "revoke", "--token-url", TOKEN_URL],
    ]
}

fn validates(args: &[&str]) -> bool {
    let cli = Cli::try_parse_from(std::iter::once("lpcli").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("{args:?} must parse: {error}"));
    validate_cli(&cli).is_ok()
}

#[test]
fn every_command_supports_json_and_only_lists_support_tsv() {
    let lists = |args: &[&str]| {
        matches!(
            args,
            ["webhook", "list" | "deliveries", ..]
                | ["snap", "find" | "builds", ..]
                | ["access-token", "list" | "list-git", ..]
        )
    };
    for args in read_commands().into_iter().chain(mutation_commands()) {
        let command = parse(&args).command;
        assert!(webhook_snap_token_supports_json(&command), "{args:?}");
        assert_eq!(
            webhook_snap_token_supports_tsv(&command),
            lists(&args),
            "{args:?}"
        );
        assert!(
            validates(&[&args[..], &["--format", "json"]].concat()),
            "{args:?}"
        );
        assert_eq!(
            validates(&[&args[..], &["--format", "tsv"]].concat()),
            lists(&args),
            "{args:?} --format tsv"
        );
    }
}

#[test]
fn quiet_is_accepted_for_mutations_and_rejected_for_reads() {
    for args in mutation_commands() {
        assert!(validates(&[&args[..], &["--quiet"]].concat()), "{args:?}");
    }
    for args in read_commands() {
        assert!(!validates(&[&args[..], &["--quiet"]].concat()), "{args:?}");
    }
}

#[test]
fn field_selection_validates_for_json_reads() {
    assert!(validates(&[
        "webhook",
        "list",
        "--target",
        "launchpad",
        "--format",
        "json",
        "--fields",
        "delivery_url,active",
    ]));
    assert!(!validates(&[
        "webhook",
        "list",
        "--target",
        "launchpad",
        "--format",
        "tsv",
        "--fields",
        "delivery_url",
    ]));
}

// ---------------------------------------------------------------------------
// Webhook reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_list_prints_launchpad_collection() {
    let mut server = mockito::Server::new_async().await;
    let body = collection(vec![
        webhook_json(LP, 1, true, json!(["git:push:0.1", "merge-proposal:0.1"])),
        webhook_json(LP, 2, false, json!(null)),
    ]);
    let list = server
        .mock("GET", "/launchpad/webhooks")
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = webhook_result(&client, webhook(&["list", "--target", "launchpad"]))
        .await
        .unwrap();
    assert_read("webhook_list", &result, &body, Some(2), &server.url());

    let CommandResult::Read(report) = &result else {
        unreachable!()
    };
    let selected = Output::fixed(OutputMode::Json, false, 100)
        .with_fields(&["delivery_url", "active"])
        .render(report);
    let selected: Value = serde_json::from_str(&selected).unwrap();
    assert_eq!(
        selected,
        json!({"total_size": 2, "start": 0, "entries": [
            {"delivery_url": "https://ci.example.com/hooks/1", "active": true},
            {"delivery_url": "https://ci.example.com/hooks/2", "active": false},
        ]})
    );
    list.assert_async().await;
}

#[tokio::test]
async fn webhook_deliveries_fetches_every_page_without_a_continuation_link() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let first = delivery_json(LP, 7, json!(true), json!(200), false);
    let second = delivery_json(LP, 8, json!(null), json!(null), true);
    let page1 = server
        .mock("GET", "/launchpad/+webhook/1/deliveries")
        .with_body(
            json!({
                "total_size": 2,
                "start": 0,
                "entries": [first],
                "next_collection_link": format!("{base}/launchpad/+webhook/1/deliveries?ws.size=1&ws.start=1"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let page2 = server
        .mock("GET", "/launchpad/+webhook/1/deliveries")
        .match_query(mockito::Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_body(json!({"start": 1, "entries": [second]}).to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let url = format!("{base}/launchpad/+webhook/1");
    let result = webhook_result(&client, webhook(&["deliveries", "--webhook-url", &url]))
        .await
        .unwrap();
    let expected = json!({"total_size": 2, "start": 0, "entries": [first, second]});
    assert_read("webhook_deliveries", &result, &expected, Some(2), &base);
    page1.assert_async().await;
    page2.assert_async().await;
}

#[tokio::test]
async fn empty_lists_print_the_empty_message_and_an_empty_collection() {
    let mut server = mockito::Server::new_async().await;
    let body = collection(Vec::new());
    let list = server
        .mock("GET", "/launchpad/webhooks")
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = webhook_result(&client, webhook(&["list", "--target", "launchpad"]))
        .await
        .unwrap();
    assert_eq!(
        render(&result, OutputMode::Rich, false),
        "No webhooks found.\n"
    );
    assert_eq!(
        render(&result, OutputMode::Plain, false),
        "No webhooks found.\n"
    );
    assert_tsv_invariants(&render(&result, OutputMode::Tsv, false), 0);
    let json: Value = serde_json::from_str(&render(&result, OutputMode::Json, false)).unwrap();
    assert_eq!(json, body);
    list.assert_async().await;
}

// ---------------------------------------------------------------------------
// Webhook mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_create_prints_the_created_entry_and_quiet_prints_self_link() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let entry = webhook_json(
        &base,
        3,
        false,
        json!(["git:push:0.1", "merge-proposal:0.1"]),
    );
    let create = server
        .mock("POST", "/launchpad")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "newWebhook".into()),
            mockito::Matcher::UrlEncoded(
                "delivery_url".into(),
                "https://ci.example.com/hooks/3".into(),
            ),
            mockito::Matcher::UrlEncoded("active".into(), "false".into()),
            mockito::Matcher::UrlEncoded("secret".into(), "hook-shared-secret".into()),
        ]))
        .with_status(201)
        .with_header("Location", &format!("{base}/launchpad/+webhook/3"))
        .create_async()
        .await;
    let read_back = server
        .mock("GET", "/launchpad/+webhook/3")
        .with_body(entry.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = webhook_result(
        &client,
        webhook(&[
            "create",
            "--target",
            "launchpad",
            "--delivery-url",
            "https://ci.example.com/hooks/3",
            "--event-types",
            "git:push:0.1, merge-proposal:0.1",
            "--inactive",
            "--secret",
            "hook-shared-secret",
        ]),
    )
    .await
    .unwrap();
    let self_link = format!("{base}/launchpad/+webhook/3");
    assert_mutation("webhook_create", &result, &entry, Some(&self_link), &base);
    for mode in MODES {
        assert!(
            !render(&result, mode, false).contains("hook-shared-secret"),
            "the webhook secret must never be printed ({mode:?})"
        );
    }
    create.assert_async().await;
    read_back.assert_async().await;
}

#[tokio::test]
async fn webhook_ping_prints_the_delivery_entry() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let entry = delivery_json(&base, 9, json!(true), json!(200), false);
    let ping = server
        .mock("POST", "/launchpad/+webhook/1")
        .match_body(mockito::Matcher::UrlEncoded("ws.op".into(), "ping".into()))
        .with_status(201)
        .with_header(
            "Location",
            &format!("{base}/launchpad/+webhook/1/+delivery/9"),
        )
        .create_async()
        .await;
    let read_back = server
        .mock("GET", "/launchpad/+webhook/1/+delivery/9")
        .with_body(entry.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let url = format!("{base}/launchpad/+webhook/1");
    let result = webhook_result(&client, webhook(&["ping", "--webhook-url", &url]))
        .await
        .unwrap();
    let self_link = format!("{base}/launchpad/+webhook/1/+delivery/9");
    assert_mutation("webhook_ping", &result, &entry, Some(&self_link), &base);
    ping.assert_async().await;
    read_back.assert_async().await;
}

#[tokio::test]
async fn webhook_delete_prints_an_lpcli_object_and_quiet_prints_nothing() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let delete = server
        .mock("DELETE", "/launchpad/+webhook/1")
        .with_status(200)
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let url = format!("{base}/launchpad/+webhook/1");
    let result = webhook_result(&client, webhook(&["delete", "--webhook-url", &url]))
        .await
        .unwrap();
    let expected = json!({"lpcli": {"deleted": true, "webhook_link": url}});
    assert_mutation("webhook_delete", &result, &expected, None, &base);
    delete.assert_async().await;
}

// ---------------------------------------------------------------------------
// Snap reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snap_show_prints_the_launchpad_entry() {
    let mut server = mockito::Server::new_async().await;
    let body = snap_json(LP, "my-snap", json!("my-snap"), false);
    let show = server
        .mock("GET", "/~jdoe/+snap/my-snap")
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, false);
    let result = snap_result(
        &client,
        snap(&["show", "--owner", "jdoe", "--name", "my-snap"]),
    )
    .await
    .unwrap();
    assert_read("snap_show", &result, &body, None, &server.url());
    show.assert_async().await;
}

#[tokio::test]
async fn snap_find_prints_launchpad_collection() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let body = collection(vec![
        snap_json(LP, "my-snap", json!("my-snap"), false),
        snap_json(LP, "private-snap", json!(null), true),
    ]);
    let find = server
        .mock("GET", "/+snaps")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "findByOwner".into()),
            mockito::Matcher::UrlEncoded("owner".into(), format!("{base}/~jdoe")),
        ]))
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, false);
    let result = snap_result(&client, snap(&["find", "--owner", "jdoe"]))
        .await
        .unwrap();
    assert_read("snap_find", &result, &body, Some(2), &base);
    find.assert_async().await;
}

#[tokio::test]
async fn snap_builds_prints_launchpad_collection() {
    let mut server = mockito::Server::new_async().await;
    let body = collection(vec![
        snap_build_json(LP, 11, "amd64", 2510),
        snap_build_json(LP, 12, "arm64", 2505),
    ]);
    let builds = server
        .mock("GET", "/~jdoe/+snap/my-snap/pending_builds")
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, false);
    let result = snap_result(
        &client,
        snap(&["builds", "--owner", "jdoe", "--name", "my-snap"]),
    )
    .await
    .unwrap();
    assert_read("snap_builds", &result, &body, Some(2), &server.url());
    builds.assert_async().await;
}

// ---------------------------------------------------------------------------
// Snap mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snap_request_builds_prints_the_build_request_entry() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let entry = json!({
        "builds_collection_link": format!("{base}/~jdoe/+snap/my-snap/+build-request/5/builds"),
        "date_finished": null,
        "date_requested": TIMESTAMP,
        "error_message": null,
        "http_etag": "\"etag-request\"",
        "resource_type_link": format!("{base}/#snap_build_request"),
        "self_link": format!("{base}/~jdoe/+snap/my-snap/+build-request/5"),
        "snap_link": format!("{base}/~jdoe/+snap/my-snap"),
        "status": "Pending",
        "web_link": "https://launchpad.net/~jdoe/+snap/my-snap/+build-request/5",
    });
    let request = server
        .mock("POST", "/~jdoe/+snap/my-snap")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "requestBuilds".into()),
            mockito::Matcher::UrlEncoded(
                "archive".into(),
                format!("{base}/ubuntu/+archive/primary"),
            ),
            mockito::Matcher::UrlEncoded("pocket".into(), "Updates".into()),
        ]))
        .with_status(201)
        .with_header(
            "Location",
            &format!("{base}/~jdoe/+snap/my-snap/+build-request/5"),
        )
        .create_async()
        .await;
    let read_back = server
        .mock("GET", "/~jdoe/+snap/my-snap/+build-request/5")
        .with_body(entry.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = snap_result(
        &client,
        snap(&[
            "request-builds",
            "--owner",
            "jdoe",
            "--name",
            "my-snap",
            "--pocket",
            "updates",
        ]),
    )
    .await
    .unwrap();
    let self_link = format!("{base}/~jdoe/+snap/my-snap/+build-request/5");
    assert_mutation(
        "snap_request_builds",
        &result,
        &entry,
        Some(&self_link),
        &base,
    );
    request.assert_async().await;
    read_back.assert_async().await;
}

// ---------------------------------------------------------------------------
// Access-token reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn access_token_list_prints_launchpad_collection() {
    let mut server = mockito::Server::new_async().await;
    let body = collection(vec![
        access_token_json(LP, "launchpad", 1, json!(TIMESTAMP)),
        access_token_json(LP, "launchpad", 2, json!(null)),
    ]);
    let list = server
        .mock("GET", "/launchpad")
        .match_query(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "getAccessTokens".into(),
        ))
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = access_token_result(&client, access_token(&["list", "--project", "launchpad"]))
        .await
        .unwrap();
    assert_read("access_token_list", &result, &body, Some(2), &server.url());
    list.assert_async().await;
}

#[tokio::test]
async fn access_token_list_git_prints_launchpad_collection() {
    let mut server = mockito::Server::new_async().await;
    let repo = "~jdoe/launchpad/+git/repo";
    let body = collection(vec![access_token_json(LP, repo, 4, json!(null))]);
    let list = server
        .mock("GET", "/~jdoe/launchpad/+git/repo")
        .match_query(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "getAccessTokens".into(),
        ))
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = access_token_result(&client, access_token(&["list-git", "--repo", repo]))
        .await
        .unwrap();
    assert_read(
        "access_token_list_git",
        &result,
        &body,
        Some(1),
        &server.url(),
    );
    list.assert_async().await;
}

// ---------------------------------------------------------------------------
// Access-token mutations
// ---------------------------------------------------------------------------

/// The secret is the only result, so every mode, and `--quiet`, prints it.
fn assert_issued_secret(name: &str, result: &CommandResult, base: &str) {
    let expected = json!({"lpcli": {"secret": TOKEN_SECRET}});
    for mode in MODES {
        if mode != OutputMode::Tsv {
            assert!(
                render(result, mode, false).contains(TOKEN_SECRET),
                "{name} must print the secret in {mode:?}"
            );
        }
    }
    assert_mutation(name, result, &expected, Some(TOKEN_SECRET), base);
}

#[tokio::test]
async fn access_token_issue_prints_the_secret_even_when_quiet() {
    let mut server = mockito::Server::new_async().await;
    let issue = server
        .mock("POST", "/launchpad")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "issueAccessToken".into()),
            mockito::Matcher::UrlEncoded("description".into(), "CI token".into()),
            // Repeated keys: UrlEncoded matches only one value per key.
            mockito::Matcher::Regex(
                "scopes=repository%3Apush&scopes=repository%3Abuild_status".into(),
            ),
        ]))
        .with_header("Content-Type", "application/json")
        .with_body(json!(TOKEN_SECRET).to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = access_token_result(
        &client,
        access_token(&[
            "issue",
            "--project",
            "launchpad",
            "--description",
            "CI token",
            "--scopes",
            "repository:push, repository:build_status",
        ]),
    )
    .await
    .unwrap();
    assert_issued_secret("access_token_issue", &result, &server.url());
    issue.assert_async().await;
}

#[tokio::test]
async fn access_token_issue_git_prints_the_secret_even_when_quiet() {
    let mut server = mockito::Server::new_async().await;
    let issue = server
        .mock("POST", "/~jdoe/launchpad/+git/repo")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("ws.op".into(), "issueAccessToken".into()),
            mockito::Matcher::UrlEncoded("description".into(), "CI token".into()),
            mockito::Matcher::UrlEncoded("scopes".into(), "repository:push".into()),
        ]))
        .with_header("Content-Type", "application/json")
        .with_body(json!(TOKEN_SECRET).to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let result = access_token_result(
        &client,
        access_token(&[
            "issue-git",
            "--repo",
            "~jdoe/launchpad/+git/repo",
            "--description",
            "CI token",
            "--scopes",
            "repository:push",
        ]),
    )
    .await
    .unwrap();
    assert_issued_secret("access_token_issue_git", &result, &server.url());
    issue.assert_async().await;
}

#[tokio::test]
async fn access_token_revoke_prints_an_lpcli_object_and_quiet_prints_nothing() {
    let mut server = mockito::Server::new_async().await;
    let base = server.url();
    let revoke = server
        .mock("POST", "/launchpad/+access-token/1")
        .match_body(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "revoke".into(),
        ))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let url = format!("{base}/launchpad/+access-token/1");
    let result = access_token_result(&client, access_token(&["revoke", "--token-url", &url]))
        .await
        .unwrap();
    let expected = json!({"lpcli": {"revoked": true, "access_token_link": url}});
    assert_mutation("access_token_revoke", &result, &expected, None, &base);
    revoke.assert_async().await;
}

#[test]
fn quiet_lines_cannot_inject_terminal_controls() {
    let result = CommandResult::Mutation {
        report: Report::json(json!({})),
        quiet: Some("https://example.com/\u{1b}[31mred\nline".into()),
    };
    assert_eq!(
        render(&result, OutputMode::Rich, true),
        "https://example.com/\u{FFFD}[31mred\u{FFFD}line\n"
    );
}
