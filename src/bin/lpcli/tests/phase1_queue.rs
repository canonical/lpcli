//! Phase 1 CLI tests for queue commands.
//!
//! - `queue search` prints Launchpad's single result page unchanged, keeping
//!   `next_collection_link` when `--limit` cut the results short, and is the
//!   only queue command with `tsv` output.
//! - `queue info` and `queue download` print lpcli-defined objects whose
//!   members are unmodified Launchpad responses.
//! - `queue download` returns `LpError::Partial` (exit 7) after printing its
//!   results when only some files fail, and a general error when all fail.
//! - `queue accept` and `queue reject` print the upload read back from
//!   Launchpad, or with `--quiet` only its `self_link`.
//!
//! Snapshots redact mock-server URLs, temporary paths, and timestamps.

#[allow(unused_imports)]
use super::*;
use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use lpcli::error::LpError;
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";

/// Replace the production API root in every string, so links in mocked
/// bodies point at the mock server.
fn rebase(value: Value, base: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(LP, base)),
        Value::Array(values) => values.into_iter().map(|v| rebase(v, base)).collect(),
        Value::Object(map) => map
            .into_iter()
            .map(|(key, value)| (key, rebase(value, base)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other,
    }
}

/// The JSON with the mock-server URL replaced by the production API root,
/// so snapshots do not depend on the server's port.
fn stable(value: &Value, base: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(base, LP)),
        Value::Array(values) => values.iter().map(|v| stable(v, base)).collect(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), stable(value, base)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
}

macro_rules! assert_json_report {
    ($name:expr, $value:expr) => {
        insta::assert_json_snapshot!($name, $value, {
            ".**.date_created" => "[timestamp]",
            ".lpcli.output_directory" => "[output directory]",
            ".lpcli.downloads[].path" => "[output path]",
        })
    };
}

fn render(report: &Report, mode: OutputMode) -> String {
    Output::fixed(mode, false, 100).render(report)
}

fn assert_modes(report: &Report) {
    let json: Value = serde_json::from_str(&render(report, OutputMode::Json))
        .expect("json output must always parse");
    assert_eq!(json, report.json);
    assert_plain_invariants(&render(report, OutputMode::Plain));
    assert!(!render(report, OutputMode::Rich).contains('\u{1b}'));
}

/// A Launchpad `package_upload` entry, including fields lpcli does not model.
fn upload_json(id: u64, name: &str, version: &str, arches: &str, status: &str) -> Value {
    json!({
        "self_link": format!("{LP}/ubuntu/noble/+upload/{id}"),
        "web_link": format!("https://launchpad.net/ubuntu/noble/+upload/{id}"),
        "resource_type_link": format!("{LP}/#package_upload"),
        "http_etag": "\"etag-upload\"",
        "id": id,
        "package_name": name,
        "package_version": version,
        "display_name": name,
        "display_version": version,
        "display_arches": arches,
        "component_name": "main",
        "section_name": "web",
        "pocket": "Proposed",
        "status": status,
        "archive_link": format!("{LP}/ubuntu/+archive/primary"),
        "distroseries_link": format!("{LP}/ubuntu/noble"),
        "changes_file_url": format!("https://launchpad.net/ubuntu/noble/+upload/{id}/+files/{name}_{version}_source.changes"),
        "contains_source": arches == "source",
        "contains_build": arches != "source",
        "contains_copy": false,
        "copy_source_archive_link": null,
        "custom_file_urls": [],
        "logs_collection_link": format!("{LP}/ubuntu/noble/+upload/{id}/logs"),
        "date_created": "2026-04-05T06:07:08.901234+00:00",
    })
}

fn noble_archive(client: &LaunchpadClient) -> QueueArchive {
    resolve_queue_archive(client, "ubuntu", None, None).unwrap()
}

fn uploads_query(archive: &QueueArchive, name: &str, status: &str) -> Matcher {
    Matcher::AllOf(vec![
        Matcher::UrlEncoded("ws.op".into(), "getPackageUploads".into()),
        Matcher::UrlEncoded("archive".into(), archive.api_url.clone()),
        Matcher::UrlEncoded("name".into(), name.into()),
        Matcher::UrlEncoded("status".into(), status.into()),
        Matcher::UrlEncoded("exact_match".into(), "true".into()),
    ])
}

fn queue_command(argv: &[&str]) -> QueueCommand {
    let mut full = vec!["lpcli", "queue"];
    full.extend(argv);
    match Cli::try_parse_from(full).unwrap().command {
        Command::Queue(command) => command,
        other => panic!("unexpected command: {other:?}"),
    }
}

fn validates(argv: &[&str]) -> bool {
    Cli::try_parse_from(argv)
        .map(|cli| validate_cli(&cli).is_ok())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Supported modes and flags
// ---------------------------------------------------------------------------

const INFO: &[&str] = &[
    "info", "--name", "curl", "--status", "new", "--arch", "source",
];
const DOWNLOAD: &[&str] = &[
    "download", "--name", "curl", "--status", "new", "--arch", "source",
];
const ACCEPT: &[&str] = &["accept", "--name", "curl", "--arch", "source"];
const REJECT: &[&str] = &["reject", "--name", "curl", "--arch", "source"];
const SEARCH: &[&str] = &["search", "--status", "new"];

#[test]
fn every_queue_command_supports_json_and_only_search_supports_tsv() {
    for argv in [SEARCH, INFO, DOWNLOAD, ACCEPT, REJECT] {
        let command = queue_command(argv);
        let list = matches!(command, QueueCommand::Search { .. });
        assert!(queue_supports_json(&command), "{argv:?}");
        assert_eq!(queue_supports_tsv(&command), list, "{argv:?}");
        for (format, supported) in [("json", true), ("tsv", list)] {
            let full = [&["lpcli", "--format", format, "queue"][..], argv].concat();
            assert_eq!(validates(&full), supported, "{full:?}");
        }
    }
}

#[test]
fn quiet_is_accepted_for_accept_and_reject_and_rejected_for_reads() {
    for argv in [ACCEPT, REJECT] {
        assert!(
            validates(&[&["lpcli", "--quiet", "queue"][..], argv].concat()),
            "{argv:?}"
        );
    }
    for argv in [SEARCH, INFO, DOWNLOAD] {
        assert!(
            !validates(&[&["lpcli", "--quiet", "queue"][..], argv].concat()),
            "{argv:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// queue search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queue_search_prints_the_launchpad_page_and_keeps_the_continuation() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let body = json!({
        "total_size": 3,
        "start": 0,
        "entries": [
            rebase(upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "New"), &base),
            rebase(upload_json(2, "curl", "8.5.0-2ubuntu10", "amd64, arm64", "New"), &base),
        ],
        "next_collection_link": format!("{base}/ubuntu/noble?ws.op=getPackageUploads&ws.size=2&memo=2&ws.start=2"),
    });
    let mock = server
        .mock("GET", "/ubuntu/noble")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getPackageUploads".into()),
            Matcher::UrlEncoded("archive".into(), archive.api_url.clone()),
            Matcher::UrlEncoded("status".into(), "New".into()),
            Matcher::UrlEncoded("pocket".into(), "Proposed".into()),
            Matcher::UrlEncoded("ws.size".into(), "2".into()),
        ]))
        .with_body(body.to_string())
        .expect(1)
        .create_async()
        .await;

    let search = queue_search(
        &client,
        Some(Pocket::Proposed),
        Some("New"),
        None,
        None,
        false,
        &archive,
        "ubuntu",
        Some("noble".into()),
        2,
    )
    .await
    .unwrap();
    let report = queue_search_report(&search);
    assert_eq!(
        report.json, body,
        "JSON must be the Launchpad page exactly, without following next_collection_link"
    );
    assert_modes(&report);
    let tsv = render(&report, OutputMode::Tsv);
    assert_tsv_invariants(&tsv, 2);
    assert!(tsv.starts_with(
        "package_name\tpackage_version\tcomponent_name\tpocket\tstatus\tdisplay_arches\tdate_created\n"
    ));
    let plain = render(&report, OutputMode::Plain);
    assert!(plain.starts_with("Upload queue for ubuntu/noble\n2 uploads found."));
    assert!(plain.contains("raise --limit"), "{plain}");
    assert_json_report!("queue_search_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn queue_search_resolves_the_development_series_and_shows_the_ppa() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, true);
    let ppa = parse_ppa_ref("owner/test").unwrap();
    let archive = resolve_queue_archive(&client, "ubuntu", None, Some(&ppa)).unwrap();
    let distro = server
        .mock("GET", "/ubuntu")
        .with_body(json!({"current_series_link": format!("{base}/ubuntu/tenacious")}).to_string())
        .create_async()
        .await;
    let body = json!({"total_size": 0, "start": 0, "entries": []});
    let uploads = server
        .mock("GET", "/ubuntu/tenacious")
        .match_query(Matcher::UrlEncoded(
            "archive".into(),
            archive.api_url.clone(),
        ))
        .with_body(body.to_string())
        .create_async()
        .await;
    let search = queue_search(
        &client,
        None,
        Some("Done"),
        Some("mutter"),
        None,
        false,
        &archive,
        "ubuntu",
        None,
        30,
    )
    .await
    .unwrap();
    let report = queue_search_report(&search);
    assert_eq!(report.json, body);
    assert_eq!(
        render(&report, OutputMode::Plain),
        "Upload queue for ubuntu/tenacious (~owner/test)\nNo packages found in the queue.\n"
    );
    assert_tsv_invariants(&render(&report, OutputMode::Tsv), 0);
    distro.assert_async().await;
    uploads.assert_async().await;
}

// ---------------------------------------------------------------------------
// queue info
// ---------------------------------------------------------------------------

fn binary_json(name: &str, arch: &str) -> Value {
    json!({
        "name": name,
        "version": "8.5.0-2ubuntu10",
        "architecture": arch,
        "component": "main",
        "section": "web",
        "priority": "optional",
        "is_new": false,
        "customformat": "",
    })
}

#[tokio::test]
async fn queue_info_is_an_lpcli_object_with_upload_binaries_and_rejection_logs() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let skipped = rebase(
        upload_json(2, "curl", "8.5.0-2ubuntu9", "source", "Rejected"),
        &base,
    );
    let upload = rebase(
        upload_json(3, "curl", "8.5.0-2ubuntu10", "amd64", "Rejected"),
        &base,
    );
    let uploads = server
        .mock("GET", "/ubuntu/noble")
        .match_query(uploads_query(&archive, "curl", "Rejected"))
        .with_body(json!({"total_size": 2, "start": 0, "entries": [skipped, upload]}).to_string())
        .create_async()
        .await;
    let log = json!({
        "self_link": format!("{base}/ubuntu/noble/+upload/3/logs/1"),
        "resource_type_link": format!("{base}/#package_upload_log"),
        "http_etag": "\"etag-log\"",
        "reviewer_link": format!("{base}/~archive-admin"),
        "old_status": "New",
        "new_status": "Rejected",
        "comment": "Needs a bug reference.",
        "date_created": "2026-04-06T07:08:09.012345+00:00",
    });
    let logs = server
        .mock("GET", "/ubuntu/noble/+upload/3/logs")
        .with_body(json!({"total_size": 1, "start": 0, "entries": [log]}).to_string())
        .create_async()
        .await;
    let binaries_body = json!([
        binary_json("curl", "amd64"),
        binary_json("libcurl4t64", "amd64"),
        binary_json("curl-dbgsym", "amd64"),
    ]);
    let binaries = server
        .mock("GET", "/ubuntu/noble/+upload/3")
        .match_query(Matcher::UrlEncoded(
            "ws.op".into(),
            "getBinaryProperties".into(),
        ))
        .with_body(binaries_body.to_string())
        .create_async()
        .await;

    let info = queue_info(
        &client,
        "curl",
        None,
        "Rejected",
        QueueArch::Arch(Architecture::Amd64),
        &archive,
        "ubuntu",
        Some("noble".into()),
    )
    .await
    .unwrap();
    let json = info.json();
    assert_eq!(
        json["package_upload"], upload,
        "the matching entry, unchanged"
    );
    assert_eq!(json["binary_properties"], binaries_body);
    assert_eq!(
        json["logs"],
        json!({"total_size": 1, "start": 0, "entries": [log]})
    );
    assert_eq!(
        json["lpcli"],
        json!({"archive": "ubuntu", "series": "noble"})
    );

    let output = Output::fixed(OutputMode::Json, false, 100);
    assert_eq!(
        serde_json::from_str::<Value>(&info.render(&output)).unwrap(),
        json
    );
    let plain = info.render(&Output::fixed(OutputMode::Plain, false, 100));
    assert_plain_invariants(&plain);
    assert!(
        plain.contains("Reason: Needs a bug reference.\n"),
        "{plain}"
    );
    assert!(plain.contains("2 binary packages found."), "{plain}");
    assert!(plain.contains("1 debug symbol package found."), "{plain}");
    assert!(
        plain.contains("Total: 3 binary package(s) (2 .deb, 1 .ddeb)"),
        "{plain}"
    );
    let rich = info.render(&Output::fixed(OutputMode::Rich, false, 100));
    assert!(!rich.contains('\u{1b}'));
    assert_json_report!("queue_info_json", stable(&json, &base));
    uploads.assert_async().await;
    logs.assert_async().await;
    binaries.assert_async().await;
}

#[tokio::test]
async fn queue_info_for_a_source_upload_has_no_logs_member() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let upload = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "New"),
        &base,
    );
    let uploads = server
        .mock("GET", "/ubuntu/noble")
        .match_query(uploads_query(&archive, "curl", "New"))
        .with_body(json!({"start": 0, "entries": [upload]}).to_string())
        .create_async()
        .await;
    let binaries = server
        .mock("GET", "/ubuntu/noble/+upload/1")
        .match_query(Matcher::UrlEncoded(
            "ws.op".into(),
            "getBinaryProperties".into(),
        ))
        .with_body("[]")
        .create_async()
        .await;
    let info = queue_info(
        &client,
        "curl",
        None,
        "New",
        QueueArch::Source,
        &archive,
        "ubuntu",
        Some("noble".into()),
    )
    .await
    .unwrap();
    let json = info.json();
    assert!(json.get("logs").is_none(), "{json}");
    assert_eq!(json["binary_properties"], json!([]));
    let plain = info.render(&Output::fixed(OutputMode::Plain, false, 100));
    assert!(
        plain.contains("No binary packages (.deb/.ddeb) found for this upload."),
        "{plain}"
    );
    uploads.assert_async().await;
    binaries.assert_async().await;
}

// ---------------------------------------------------------------------------
// queue download
// ---------------------------------------------------------------------------

/// Mock an upload with two source files; `fail` makes the second 404.
async fn mock_download(
    server: &mut mockito::Server,
    archive: &QueueArchive,
    fail: &[&str],
) -> (Vec<mockito::Mock>, Value) {
    let base = server.url();
    let upload = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "New"),
        &base,
    );
    let urls = json!([
        format!("{base}/files/curl_8.5.0-2ubuntu10.dsc"),
        format!("{base}/files/curl_8.5.0.orig.tar.gz"),
    ]);
    let mut mocks = vec![
        server
            .mock("GET", "/ubuntu/noble")
            .match_query(uploads_query(archive, "curl", "New"))
            .with_body(json!({"start": 0, "entries": [upload]}).to_string())
            .create_async()
            .await,
        server
            .mock("GET", "/ubuntu/noble/+upload/1")
            .match_query(Matcher::UrlEncoded("ws.op".into(), "sourceFileUrls".into()))
            .with_body(urls.to_string())
            .create_async()
            .await,
    ];
    for file in ["curl_8.5.0-2ubuntu10.dsc", "curl_8.5.0.orig.tar.gz"] {
        let mock = server
            .mock("GET", format!("/files/{file}").as_str())
            .match_header("authorization", Matcher::Missing);
        let mock = if fail.contains(&file) {
            mock.with_status(404).with_body("Not found")
        } else {
            mock.with_body(format!("contents of {file}"))
        };
        mocks.push(mock.create_async().await);
    }
    (mocks, urls)
}

#[tokio::test]
async fn queue_download_is_an_lpcli_object_and_succeeds_when_every_file_downloads() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let (mocks, urls) = mock_download(&mut server, &archive, &[]).await;
    let dir = tempfile::tempdir().unwrap();

    let target = queue_download_target(
        &client,
        "curl",
        None,
        "New",
        QueueArch::Source,
        &archive,
        "ubuntu",
        Some("noble".into()),
    )
    .await
    .unwrap();
    assert!(
        target.binary_file_urls.is_none(),
        "not requested for sources"
    );
    let outcomes = download_public_files(&target.urls(), dir.path())
        .await
        .unwrap();
    let download = QueueDownload {
        target,
        output_dir: dir.path().to_path_buf(),
        outcomes,
    };
    download.status().unwrap();
    let report = download.report();
    assert_eq!(report.json["source_file_urls"], urls);
    assert!(report.json.get("binary_file_urls").is_none());
    assert_eq!(report.json["lpcli"]["downloaded"], 2);
    assert_eq!(report.json["lpcli"]["failed"], 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("curl_8.5.0-2ubuntu10.dsc")).unwrap(),
        "contents of curl_8.5.0-2ubuntu10.dsc"
    );
    assert_modes(&report);
    assert_json_report!("queue_download_json", stable(&report.json, &base));
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn queue_download_partial_failures_return_partial_after_printing() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let (mocks, _) = mock_download(&mut server, &archive, &["curl_8.5.0.orig.tar.gz"]).await;
    let dir = tempfile::tempdir().unwrap();

    let err = handle_queue_download(
        &Output::fixed(OutputMode::Json, false, 100),
        &client,
        "curl".into(),
        None,
        "New".into(),
        QueueArch::Source,
        &archive,
        "ubuntu".into(),
        Some("noble".into()),
        dir.path().to_string_lossy().into_owned(),
    )
    .await
    .unwrap_err();
    match &err {
        LpError::Partial(message) => {
            assert!(message.contains("Downloaded 1 of 2"), "{message}");
            assert!(message.contains("curl_8.5.0.orig.tar.gz"), "{message}");
        }
        other => panic!("expected a partial failure, got {other:?}"),
    }
    assert_eq!(err.exit_code(), 7);
    assert!(dir.path().join("curl_8.5.0-2ubuntu10.dsc").exists());
    assert!(!dir.path().join("curl_8.5.0.orig.tar.gz").exists());

    // The printed report lists both outcomes.
    let target = queue_download_target(
        &client,
        "curl",
        None,
        "New",
        QueueArch::Source,
        &archive,
        "ubuntu",
        Some("noble".into()),
    )
    .await
    .unwrap();
    let download = QueueDownload {
        outcomes: download_public_files(&target.urls(), dir.path())
            .await
            .unwrap(),
        target,
        output_dir: dir.path().to_path_buf(),
    };
    let report = download.report();
    assert_eq!(report.json["lpcli"]["downloaded"], 1);
    assert_eq!(report.json["lpcli"]["failed"], 1);
    let plain = render(&report, OutputMode::Plain);
    assert_plain_invariants(&plain);
    assert!(plain.contains("Error: curl_8.5.0.orig.tar.gz: "), "{plain}");
    assert!(plain.contains("1 file(s) downloaded from ubuntu/noble, 1 failed."));
    assert_json_report!("queue_download_partial_json", stable(&report.json, &base));
    drop(mocks);
}

#[tokio::test]
async fn queue_download_where_every_file_fails_is_a_general_error() {
    let mut server = isolated_mock_server().await;
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let (_mocks, _) = mock_download(
        &mut server,
        &archive,
        &["curl_8.5.0-2ubuntu10.dsc", "curl_8.5.0.orig.tar.gz"],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let err = handle_queue_download(
        &Output::fixed(OutputMode::Json, false, 100),
        &client,
        "curl".into(),
        None,
        "New".into(),
        QueueArch::Source,
        &archive,
        "ubuntu".into(),
        Some("noble".into()),
        dir.path().to_string_lossy().into_owned(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, LpError::Other(_)), "{err:?}");
    assert_eq!(err.exit_code(), 1);
}

#[tokio::test]
async fn queue_download_of_a_missing_upload_is_not_found() {
    let mut server = isolated_mock_server().await;
    let client = mock_read_client(&server, false);
    let archive = noble_archive(&client);
    let uploads = server
        .mock("GET", "/ubuntu/noble")
        .match_query(uploads_query(&archive, "curl", "New"))
        .with_body(r#"{"start": 0, "entries": []}"#)
        .create_async()
        .await;
    let err = handle_queue_download(
        &Output::fixed(OutputMode::Json, false, 100),
        &client,
        "curl".into(),
        None,
        "New".into(),
        QueueArch::Source,
        &archive,
        "ubuntu".into(),
        Some("noble".into()),
        ".".into(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, LpError::LocalNotFound(_)), "{err:?}");
    uploads.assert_async().await;
}

// ---------------------------------------------------------------------------
// queue accept and reject
// ---------------------------------------------------------------------------

/// Mock finding upload 1, the queue operation, and the read-back.
async fn mock_mutation(
    server: &mut mockito::Server,
    archive: &QueueArchive,
    operation: Matcher,
    refreshed_status: &str,
) -> (Vec<mockito::Mock>, Value) {
    let base = server.url();
    let found = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "Unapproved"),
        &base,
    );
    let refreshed = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", refreshed_status),
        &base,
    );
    let mocks = vec![
        server
            .mock("GET", "/ubuntu/noble")
            .match_query(uploads_query(archive, "curl", "Unapproved"))
            .with_body(json!({"start": 0, "entries": [found]}).to_string())
            .create_async()
            .await,
        server
            .mock("POST", "/ubuntu/noble/+upload/1")
            .match_body(operation)
            .with_status(200)
            .with_body("null")
            .create_async()
            .await,
        server
            .mock("GET", "/ubuntu/noble/+upload/1")
            .match_query(Matcher::Missing)
            .with_body(refreshed.to_string())
            .create_async()
            .await,
    ];
    (mocks, refreshed)
}

#[tokio::test]
async fn queue_accept_prints_the_refreshed_entry_and_quiet_prints_its_self_link() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, true);
    let archive = noble_archive(&client);
    let (mocks, refreshed) = mock_mutation(
        &mut server,
        &archive,
        Matcher::UrlEncoded("ws.op".into(), "acceptFromQueue".into()),
        "Accepted",
    )
    .await;

    let mutation = handle_queue_accept(
        &client,
        "curl".into(),
        None,
        "Unapproved".into(),
        QueueArch::Source,
        &archive,
        "ubuntu".into(),
        Some("noble".into()),
    )
    .await
    .unwrap();
    let entry = refresh_queue_upload(&client, &mutation).await.unwrap();
    assert_eq!(entry.raw, refreshed, "the entry read back, unchanged");

    let report = queue_mutation_report(&mutation, &entry);
    assert_eq!(report.json, refreshed);
    assert_modes(&report);
    let plain = render(&report, OutputMode::Plain);
    assert!(
        plain.starts_with("OK: Accepted curl (8.5.0-2ubuntu10) [source] in ubuntu/noble.\n"),
        "{plain}"
    );
    assert!(plain.contains("Status: Accepted\n"), "{plain}");
    for mode in [OutputMode::Rich, OutputMode::Plain, OutputMode::Json] {
        assert_eq!(
            render_queue_mutation(&Output::fixed(mode, false, 100), true, &mutation, &entry),
            format!("{base}/ubuntu/noble/+upload/1\n"),
            "--quiet in {mode:?}"
        );
    }
    assert_json_report!("queue_accept_json", stable(&report.json, &base));
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn queue_reject_prints_the_refreshed_entry_and_quiet_prints_its_self_link() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, true);
    let archive = noble_archive(&client);
    let (mocks, refreshed) = mock_mutation(
        &mut server,
        &archive,
        Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "rejectFromQueue".into()),
            Matcher::UrlEncoded("comment".into(), "Needs a bug fix.".into()),
        ]),
        "Rejected",
    )
    .await;

    let mutation = handle_queue_reject(
        &client,
        "curl".into(),
        None,
        "Unapproved".into(),
        QueueArch::Source,
        &archive,
        Some("Needs a bug fix.".into()),
        "ubuntu".into(),
        Some("noble".into()),
    )
    .await
    .unwrap();
    let entry = refresh_queue_upload(&client, &mutation).await.unwrap();
    let report = queue_mutation_report(&mutation, &entry);
    assert_eq!(report.json, refreshed);
    assert_modes(&report);
    let plain = render(&report, OutputMode::Plain);
    assert!(plain.contains("Rejected curl (8.5.0-2ubuntu10)"), "{plain}");
    assert!(plain.contains("Comment: Needs a bug fix.\n"), "{plain}");
    assert_eq!(
        render_queue_mutation(
            &Output::fixed(OutputMode::Json, false, 100),
            true,
            &mutation,
            &entry
        ),
        format!("{base}/ubuntu/noble/+upload/1\n")
    );
    assert_json_report!("queue_reject_json", stable(&report.json, &base));
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn queue_mutation_read_back_failure_is_partial() {
    let mut server = isolated_mock_server().await;
    let client = mock_read_client(&server, true);
    let archive = noble_archive(&client);
    let base = server.url();
    let found = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "Unapproved"),
        &base,
    );
    let _uploads = server
        .mock("GET", "/ubuntu/noble")
        .match_query(uploads_query(&archive, "curl", "Unapproved"))
        .with_body(json!({"start": 0, "entries": [found]}).to_string())
        .create_async()
        .await;
    let accept = server
        .mock("POST", "/ubuntu/noble/+upload/1")
        .with_status(200)
        .create_async()
        .await;
    let refresh = server
        .mock("GET", "/ubuntu/noble/+upload/1")
        .match_query(Matcher::Missing)
        .with_status(404)
        .with_body("Gone")
        .create_async()
        .await;
    let mutation = handle_queue_accept(
        &client,
        "curl".into(),
        None,
        "Unapproved".into(),
        QueueArch::Source,
        &archive,
        "ubuntu".into(),
        Some("noble".into()),
    )
    .await
    .unwrap();
    let err = refresh_queue_upload(&client, &mutation).await.unwrap_err();
    match &err {
        LpError::Partial(message) => {
            assert!(
                message.starts_with("Accepting the upload succeeded"),
                "{message}"
            )
        }
        other => panic!("expected a partial failure, got {other:?}"),
    }
    assert_eq!(err.exit_code(), 7);
    accept.assert_async().await;
    refresh.assert_async().await;
}

#[test]
fn quiet_prints_nothing_without_a_self_link_and_neutralises_controls() {
    let upload: queue::PackageUpload =
        serde_json::from_value(upload_json(1, "curl", "1", "source", "Accepted")).unwrap();
    let mutation = QueueMutation {
        action: QueueAction::Accept,
        upload,
        self_link: format!("{LP}/ubuntu/noble/+upload/1"),
        target: "ubuntu/noble".into(),
        comment: None,
    };
    let output = Output::fixed(OutputMode::Rich, false, 100);
    let without: Raw<queue::PackageUpload> =
        Raw::from_value(json!({"status": "Accepted"})).unwrap();
    assert_eq!(
        render_queue_mutation(&output, true, &mutation, &without),
        ""
    );
    let hostile: Raw<queue::PackageUpload> =
        Raw::from_value(json!({"self_link": "https://example.com/\u{1b}[31mred\nline"})).unwrap();
    let line = render_queue_mutation(&output, true, &mutation, &hostile);
    assert_eq!(line.lines().count(), 1, "{line:?}");
    assert!(!line.contains('\u{1b}'), "{line:?}");
}

#[test]
fn queue_download_redacts_secrets_in_failed_outcomes() {
    let upload_val = upload_json(1, "curl", "1.0", "source", "New");
    let upload: queue::PackageUpload = serde_json::from_value(upload_val.clone()).unwrap();
    let download = QueueDownload {
        target: QueueDownloadTarget {
            target: "ubuntu/noble".into(),
            archive_label: "ubuntu/noble".into(),
            series: "noble".into(),
            upload: Raw {
                value: upload,
                raw: upload_val,
            },
            source_file_urls: None,
            binary_file_urls: None,
        },
        output_dir: std::path::PathBuf::from("/tmp/out"),
        outcomes: vec![download::DownloadOutcome::Failed {
            filename: "curl.dsc?oauth_token=urlsecret".into(),
            error: "HTTP 403 {'oauth_token_secret': 'pysecret'} Authorization: Bearer hdrsecret"
                .into(),
        }],
    };
    let report = download.report();
    for mode in [OutputMode::Rich, OutputMode::Plain, OutputMode::Json] {
        let rendered = Output::fixed(mode, false, 100).render(&report);
        for leaked in ["urlsecret", "pysecret", "hdrsecret"] {
            assert!(
                !rendered.contains(leaked),
                "{mode:?} leaked {leaked}: {rendered}"
            );
        }
    }
}
