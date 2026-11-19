//! Phase 4 hardening tests: closing snapshot gaps (`schema`, structured
//! errors, remaining TSV lists, `plain`/`rich` across all domains and partial
//! failures) plus final accessibility and snapshot-hygiene verification.

#[allow(unused_imports)]
use super::*;

use std::path::Path;

use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use lpcli::error::LpError;
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";

// ---------------------------------------------------------------------------
// Shared fixture and snapshot helpers
// ---------------------------------------------------------------------------

fn rebase(value: Value, base: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(LP, base)),
        Value::Array(values) => values.into_iter().map(|v| rebase(v, base)).collect(),
        Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| (k, rebase(v, base)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other,
    }
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

fn entry(kind: &str, path: &str, fields: Value) -> Value {
    let mut value = json!({
        "self_link": format!("{LP}{path}"),
        "web_link": format!("https://launchpad.net{path}"),
        "resource_type_link": format!("{LP}/#{kind}"),
        "http_etag": "\"etag-1234\"",
    });
    for (k, v) in fields.as_object().into_iter().flatten() {
        value[k] = v.clone();
    }
    value
}

fn op(name: &str) -> Matcher {
    Matcher::UrlEncoded("ws.op".into(), name.into())
}

async fn mock_get(
    server: &mut mockito::Server,
    path: &str,
    query: Matcher,
    body: &Value,
) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_query(query)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

fn render(report: &Report, mode: OutputMode) -> String {
    Output::fixed(mode, false, 100).render(report)
}

fn snap_plain(name: &str, report: &Report) {
    let rendered = render(report, OutputMode::Plain);
    assert_plain_invariants(&rendered);
    assert!(!rendered.contains("127.0.0.1:"), "{rendered}");
    insta::assert_snapshot!(name, rendered);
}

fn snap_rich(name: &str, report: &Report) {
    let rendered = render(report, OutputMode::Rich);
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(!rendered.contains("127.0.0.1:"), "{rendered}");
    insta::assert_snapshot!(name, rendered);
}

fn snap_tsv(name: &str, report: &Report) {
    let rows = report.records.rows.len();
    let rendered = render(report, OutputMode::Tsv);
    assert_tsv_invariants(&rendered, rows);
    assert!(!rendered.contains("127.0.0.1:"), "{rendered}");
    insta::assert_snapshot!(name, rendered);
}

fn redact_text(mut text: String, base: &str, dir: Option<&Path>) -> String {
    if let Some(dir) = dir {
        let dir_str = dir.to_string_lossy();
        text = text.replace(dir_str.as_ref(), "[output dir]");
    }
    text.replace(base, "[server]")
}

// ---------------------------------------------------------------------------
// Schema and structured error snapshots
// ---------------------------------------------------------------------------

#[test]
fn schema_and_error_modes_match_snapshots() {
    let schema = schema_report();
    let json: Value = serde_json::from_str(&render(&schema, OutputMode::Json)).unwrap();
    insta::assert_json_snapshot!("schema_json", json);
    snap_plain("schema_plain", &schema);
    snap_rich("schema_rich", &schema);

    let not_found = LpError::NotFound("https://api.launchpad.net/devel/bugs/999999".into());
    let not_found_json: Value =
        serde_json::from_str(&Output::fixed(OutputMode::Json, false, 100).render_error(&not_found))
            .unwrap();
    insta::assert_json_snapshot!("error_not_found_json", not_found_json);

    let partial = LpError::Partial(
        "Updated 1 of 2 task(s) on bug #42; 1 failed: set_status (curl (Ubuntu noble)): Unauthorized: permission denied oauth_token_secret=topsecret".into(),
    );
    let partial_json: Value =
        serde_json::from_str(&Output::fixed(OutputMode::Json, false, 100).render_error(&partial))
            .unwrap();
    assert!(!partial_json.to_string().contains("topsecret"));
    insta::assert_json_snapshot!("error_partial_json", partial_json);

    let unauth_plain =
        Output::fixed(OutputMode::Plain, false, 100).render_error(&LpError::NotAuthenticated);
    assert_plain_invariants(&unauth_plain);
    insta::assert_snapshot!("error_not_authenticated_plain", unauth_plain);

    let unauth_rich =
        Output::fixed(OutputMode::Rich, false, 100).render_error(&LpError::NotAuthenticated);
    assert!(!unauth_rich.contains('\u{1b}'));
    insta::assert_snapshot!("error_not_authenticated_rich", unauth_rich);
}

// ---------------------------------------------------------------------------
// Git domain TSV, plain, and rich snapshots
// ---------------------------------------------------------------------------

fn git_repo_json(name: &str) -> Value {
    json!({
        "self_link": format!("{LP}/~owner/project/+git/{name}"),
        "web_link": format!("https://code.launchpad.net/~owner/project/+git/{name}"),
        "resource_type_link": format!("{LP}/#git_repository"),
        "http_etag": "\"etag-repo\"",
        "name": name,
        "unique_name": format!("~owner/project/+git/{name}"),
        "description": "Repository description",
        "owner_default": false,
        "target_default": true,
        "repository_type": "Hosted",
        "information_type": "Public",
        "private": false,
        "owner_link": format!("{LP}/~owner"),
        "target_link": format!("{LP}/project"),
        "date_created": "2025-01-02T03:04:05+00:00",
        "date_last_modified": "2026-02-03T04:05:06+00:00",
    })
}

fn git_ref_json(path: &str, sha1: &str) -> Value {
    json!({
        "self_link": format!("{LP}/~owner/project/+git/repo/+ref/{path}"),
        "resource_type_link": format!("{LP}/#git_ref"),
        "http_etag": "\"etag-ref\"",
        "path": path,
        "display_name": path.trim_start_matches("refs/heads/"),
        "commit_sha1": sha1,
        "repository_link": format!("{LP}/~owner/project/+git/repo"),
        "web_link": null,
    })
}

fn git_proposal_json(id: u64, status: &str) -> Value {
    json!({
        "self_link": format!("{LP}/~owner/project/+git/repo/+merge/{id}"),
        "web_link": format!("https://code.launchpad.net/~owner/project/+git/repo/+merge/{id}"),
        "resource_type_link": format!("{LP}/#branch_merge_proposal"),
        "http_etag": "\"etag-mp\"",
        "queue_status": status,
        "source_git_path": "refs/heads/fix",
        "target_git_path": "refs/heads/main",
        "date_created": "2026-03-04T05:06:07+00:00",
    })
}

fn git_diff_json(id: u64) -> Value {
    json!({
        "self_link": format!("{LP}/~owner/project/+git/repo/+merge/1/+preview-diff/{id}"),
        "resource_type_link": format!("{LP}/#preview_diff"),
        "http_etag": "\"etag-diff\"",
        "id": id,
        "diff_lines_count": 12,
        "added_lines_count": 3,
        "removed_lines_count": 1,
        "source_revision_id": "0123456789abcdef0123456789abcdef01234567",
        "date_created": "2026-03-05T06:07:08+00:00",
    })
}

#[test]
fn git_tsv_plain_and_rich_snapshots() {
    let repo: Raw<git::GitRepository> = Raw::from_value(git_repo_json("repo")).unwrap();
    let show_report = git_repository_report(&repo);
    snap_plain("git_show_plain", &show_report);
    snap_rich("git_show_rich", &show_report);

    let repos_raw = collection(vec![git_repo_json("one"), git_repo_json("two")]);
    let repos_report = git_repositories_report(&Raw {
        value: serde_json::from_value(repos_raw["entries"].clone()).unwrap(),
        raw: repos_raw,
    });
    snap_tsv("git_list_person_repos_tsv", &repos_report);

    let refs_raw = collection(vec![
        git_ref_json(
            "refs/heads/main",
            "0123456789abcdef0123456789abcdef01234567",
        ),
        git_ref_json("refs/tags/v1", "fedcba9876543210fedcba9876543210fedcba98"),
    ]);
    let refs_report = git_refs_report(&Raw {
        value: serde_json::from_value(refs_raw["entries"].clone()).unwrap(),
        raw: refs_raw,
    });
    snap_tsv("git_refs_tsv", &refs_report);

    let proposals_raw = collection(vec![
        git_proposal_json(11, "Needs review"),
        git_proposal_json(12, "Approved"),
    ]);
    let proposals_report = git_proposals_report(&Raw {
        value: serde_json::from_value(proposals_raw["entries"].clone()).unwrap(),
        raw: proposals_raw,
    });
    snap_tsv("git_proposals_tsv", &proposals_report);
    snap_plain("git_proposals_plain", &proposals_report);
    snap_rich("git_proposals_rich", &proposals_report);

    let diffs_raw = collection(vec![git_diff_json(76), git_diff_json(77)]);
    let diffs_report = git_diffs_report(&Raw {
        value: serde_json::from_value(diffs_raw["entries"].clone()).unwrap(),
        raw: diffs_raw,
    });
    snap_tsv("git_diffs_tsv", &diffs_report);

    let review_raw = collection(vec![json!({
        "id": 5,
        "author_link": format!("{LP}/~reviewer"),
        "date_created": "2026-03-06T07:08:09+00:00",
        "vote": "Approve",
        "content": "Looks good,\nbut see inline.",
    })]);
    let inline_raw = json!([
        {
            "line_number": "6",
            "person": {"name": "reviewer", "self_link": format!("{LP}/~reviewer")},
            "text": "Why True?",
            "date": "2026-03-06T07:08:09+00:00",
        },
        {
            "line_number": "99",
            "person": format!("{LP}/~other"),
            "text": "Unmapped",
            "date": null,
        },
    ]);
    let diff_text = "diff --git a/lib/model.py b/lib/model.py\n--- a/lib/model.py\n+++ b/lib/model.py\n@@ -4,2 +4,3 @@\n def existing():\n+    return True\n";
    let comments = GitComments {
        comments: Raw {
            value: serde_json::from_value(review_raw["entries"].clone()).unwrap(),
            raw: review_raw,
        },
        inline: Raw::from_value(inline_raw).unwrap(),
        diff_map: git::parse_diff_line_map(diff_text),
    };
    let comments_report = git_comments_report(&comments);
    snap_plain("git_comments_plain", &comments_report);
    snap_rich("git_comments_rich", &comments_report);
}

// ---------------------------------------------------------------------------
// Queue domain TSV, plain, rich, and partial failure snapshots
// ---------------------------------------------------------------------------

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
        "logs_collection_link": format!("{LP}/ubuntu/noble/+upload/{id}/logs"),
        "date_created": "2026-04-05T06:07:00+00:00",
    })
}

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
async fn queue_tsv_plain_rich_and_partial_snapshots() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, true);
    let archive = resolve_queue_archive(&client, "ubuntu", None, None).unwrap();

    // 1. queue search
    let search_page_raw = json!({
        "total_size": 3,
        "start": 0,
        "entries": [
            rebase(upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "New"), &base),
            rebase(upload_json(2, "curl", "8.5.0-2ubuntu10", "amd64, arm64", "New"), &base),
        ],
        "next_collection_link": format!("{base}/ubuntu/noble?ws.op=getPackageUploads&ws.size=2&ws.start=2"),
    });
    let _search_mock = mock_get(
        &mut server,
        "/ubuntu/noble",
        Matcher::AllOf(vec![
            op("getPackageUploads"),
            Matcher::UrlEncoded("status".into(), "New".into()),
            Matcher::UrlEncoded("ws.size".into(), "2".into()),
        ]),
        &search_page_raw,
    )
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
    let search_report = queue_search_report(&search);
    snap_tsv("queue_search_tsv", &search_report);
    snap_plain("queue_search_plain", &search_report);
    snap_rich("queue_search_rich", &search_report);

    // 2. queue info (rejected upload with logs and .deb/.ddeb binaries)
    let rejected_upload = rebase(
        upload_json(3, "curl", "8.5.0-2ubuntu10", "amd64", "Rejected"),
        &base,
    );
    let _info_uploads = mock_get(
        &mut server,
        "/ubuntu/noble",
        Matcher::AllOf(vec![
            op("getPackageUploads"),
            Matcher::UrlEncoded("status".into(), "Rejected".into()),
        ]),
        &collection(vec![rejected_upload]),
    )
    .await;
    let _info_logs = mock_get(
        &mut server,
        "/ubuntu/noble/+upload/3/logs",
        Matcher::Missing,
        &collection(vec![json!({
            "self_link": format!("{base}/ubuntu/noble/+upload/3/logs/1"),
            "old_status": "New",
            "new_status": "Rejected",
            "comment": "Needs a bug reference.",
            "date_created": "2026-04-06T07:08:00+00:00",
        })]),
    )
    .await;
    let _info_bins = mock_get(
        &mut server,
        "/ubuntu/noble/+upload/3",
        op("getBinaryProperties"),
        &json!([
            binary_json("curl", "amd64"),
            binary_json("libcurl4t64", "amd64"),
            binary_json("curl-dbgsym", "amd64"),
        ]),
    )
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
    let info_plain = info.render(&Output::fixed(OutputMode::Plain, false, 100));
    assert_plain_invariants(&info_plain);
    insta::assert_snapshot!("queue_info_plain", info_plain);
    let info_rich = info.render(&Output::fixed(OutputMode::Rich, false, 100));
    assert!(!info_rich.contains('\u{1b}'));
    insta::assert_snapshot!("queue_info_rich", info_rich);

    // 3. queue download (success + partial)
    let new_src_upload = rebase(
        upload_json(1, "curl", "8.5.0-2ubuntu10", "source", "New"),
        &base,
    );
    let _dl_lookup = mock_get(
        &mut server,
        "/ubuntu/noble",
        Matcher::AllOf(vec![
            op("getPackageUploads"),
            Matcher::UrlEncoded("name".into(), "curl".into()),
            Matcher::UrlEncoded("status".into(), "New".into()),
            Matcher::UrlEncoded("exact_match".into(), "true".into()),
        ]),
        &collection(vec![new_src_upload]),
    )
    .await;
    let _dl_urls = mock_get(
        &mut server,
        "/ubuntu/noble/+upload/1",
        op("sourceFileUrls"),
        &json!([
            format!("{base}/files/curl_8.5.0-2ubuntu10.dsc"),
            format!("{base}/files/curl_8.5.0.orig.tar.gz"),
        ]),
    )
    .await;
    let _dl_dsc = server
        .mock("GET", "/files/curl_8.5.0-2ubuntu10.dsc")
        .with_status(200)
        .with_body("dsc-bytes")
        .create_async()
        .await;
    let _dl_orig = server
        .mock("GET", "/files/curl_8.5.0.orig.tar.gz")
        .with_status(200)
        .with_body("tarball-bytes")
        .create_async()
        .await;

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
    let ok_dir = tempfile::tempdir().unwrap();
    let ok_out_dir = ok_dir.path().join("out");
    let ok_download = QueueDownload {
        outcomes: download_public_files(&target.urls(), &ok_out_dir)
            .await
            .unwrap(),
        target: queue_download_target(
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
        .unwrap(),
        output_dir: ok_out_dir,
    };
    let ok_report = ok_download.report();
    let ok_plain = redact_text(
        render(&ok_report, OutputMode::Plain),
        &base,
        Some(ok_dir.path()),
    );
    assert_plain_invariants(&ok_plain);
    insta::assert_snapshot!("queue_download_plain", ok_plain);
    let ok_rich = redact_text(
        render(&ok_report, OutputMode::Rich),
        &base,
        Some(ok_dir.path()),
    );
    insta::assert_snapshot!("queue_download_rich", ok_rich);

    // Partial failure when second file returns 404
    drop(_dl_orig);
    let _dl_orig_404 = server
        .mock("GET", "/files/curl_8.5.0.orig.tar.gz")
        .with_status(404)
        .with_body("Not found")
        .create_async()
        .await;
    let partial_dir = tempfile::tempdir().unwrap();
    let partial_out_dir = partial_dir.path().join("out");
    let partial_download = QueueDownload {
        outcomes: download_public_files(&target.urls(), &partial_out_dir)
            .await
            .unwrap(),
        target,
        output_dir: partial_out_dir,
    };
    let partial_report = partial_download.report();
    let partial_plain = redact_text(
        render(&partial_report, OutputMode::Plain),
        &base,
        Some(partial_dir.path()),
    );
    assert_plain_invariants(&partial_plain);
    insta::assert_snapshot!("queue_download_partial_plain", partial_plain);
    let partial_rich = redact_text(
        render(&partial_report, OutputMode::Rich),
        &base,
        Some(partial_dir.path()),
    );
    insta::assert_snapshot!("queue_download_partial_rich", partial_rich);

    // 4. queue accept / reject mutations
    let unapproved = rebase(
        upload_json(4, "curl", "8.5.0-2ubuntu10", "source", "Unapproved"),
        &base,
    );
    let accepted = rebase(
        upload_json(4, "curl", "8.5.0-2ubuntu10", "source", "Accepted"),
        &base,
    );
    let rejected = rebase(
        upload_json(4, "curl", "8.5.0-2ubuntu10", "source", "Rejected"),
        &base,
    );
    let _unapproved_lookup = mock_get(
        &mut server,
        "/ubuntu/noble",
        Matcher::AllOf(vec![
            op("getPackageUploads"),
            Matcher::UrlEncoded("status".into(), "Unapproved".into()),
        ]),
        &collection(vec![unapproved]),
    )
    .await;
    let _accept_post = server
        .mock("POST", "/ubuntu/noble/+upload/4")
        .match_body(op("acceptFromQueue"))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let _accept_refresh = mock_get(
        &mut server,
        "/ubuntu/noble/+upload/4",
        Matcher::Missing,
        &accepted,
    )
    .await;

    let accept_mutation = handle_queue_accept(
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
    let accept_entry = refresh_queue_upload(&client, &accept_mutation)
        .await
        .unwrap();
    let accept_report = queue_mutation_report(&accept_mutation, &accept_entry);
    snap_plain("queue_accept_plain", &accept_report);
    snap_rich("queue_accept_rich", &accept_report);

    drop(_accept_refresh);
    let _reject_post = server
        .mock("POST", "/ubuntu/noble/+upload/4")
        .match_body(op("rejectFromQueue"))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let _reject_refresh = mock_get(
        &mut server,
        "/ubuntu/noble/+upload/4",
        Matcher::Missing,
        &rejected,
    )
    .await;

    let reject_mutation = handle_queue_reject(
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
    let reject_entry = refresh_queue_upload(&client, &reject_mutation)
        .await
        .unwrap();
    let reject_report = queue_mutation_report(&reject_mutation, &reject_entry);
    snap_plain("queue_reject_plain", &reject_report);
    snap_rich("queue_reject_rich", &reject_report);
}

// ---------------------------------------------------------------------------
// Read domains: complete TSV, plain, rich, and package download partial snaps
// ---------------------------------------------------------------------------

fn person_json(name: &str, display: &str, team: bool) -> Value {
    entry(
        if team { "team" } else { "person" },
        &format!("/~{name}"),
        json!({
            "name": name,
            "display_name": display,
            "is_team": team,
            "karma": 42,
            "description": null,
            "date_created": "2005-08-09T19:50:16+00:00",
        }),
    )
}

fn series_json(name: &str, version: &str, status: &str) -> Value {
    entry(
        "distro_series",
        &format!("/ubuntu/{name}"),
        json!({
            "name": name,
            "display_name": name,
            "version": version,
            "status": status,
            "active": true,
        }),
    )
}

fn source_json(name: &str, version: &str, id: u64) -> Value {
    entry(
        "source_package_publishing_history",
        &format!("/ubuntu/+archive/primary/+sourcepub/{id}"),
        json!({
            "source_package_name": name,
            "source_package_version": version,
            "component_name": "main",
            "section_name": "devel",
            "status": "Published",
            "pocket": "Release",
            "date_published": "2024-04-01T00:00:00+00:00",
        }),
    )
}

async fn run_read_domain(server: &mockito::Server, args: &[&str]) -> Report {
    let mut argv = vec!["lpcli"];
    argv.extend(args);
    let cli = Cli::try_parse_from(&argv).unwrap();
    validate_cli(&cli).unwrap();
    let client = mock_read_client(server, false);
    match cli.command {
        Command::Person(cmd) => person_report(&client, cmd).await,
        Command::Package(cmd) => package_report(&client, cmd).await,
        Command::Project(cmd) => project_report(&client, cmd).await,
        Command::Cve(cmd) => cve_report(&client, cmd).await,
        Command::Spec(cmd) => spec_report(&client, cmd).await,
        Command::Question(cmd) => question_report(&client, cmd).await,
        Command::Translation(cmd) => translation_report(&client, cmd).await,
        other => panic!("unexpected read-domain command: {other:?}"),
    }
    .unwrap()
}

#[tokio::test]
async fn read_domains_tsv_plain_and_rich_snapshots() {
    let mut server = isolated_mock_server().await;
    let base = server.url();

    // Person mocks
    let people_body = rebase(
        collection(vec![
            person_json("jdoe", "Jane Doe", false),
            person_json("ubuntu-dev", "Ubuntu Development Team", true),
        ]),
        &base,
    );
    let members_body = rebase(
        collection(vec![entry(
            "team_membership",
            "/~ubuntu-dev/+member/jdoe",
            json!({
                "member_link": format!("{LP}/~jdoe"),
                "team_link": format!("{LP}/~ubuntu-dev"),
                "status": "Approved",
                "date_joined": "2020-01-02T03:04:05+00:00",
            }),
        )]),
        &base,
    );
    let bugs_body = rebase(
        collection(vec![entry(
            "bug_task",
            "/ubuntu/+source/hello/+bug/7",
            json!({
                "bug_link": format!("{LP}/bugs/7"),
                "title": "Crash\twith tab",
                "status": "New",
                "importance": "Undecided",
            }),
        )]),
        &base,
    );
    let ppas_body = rebase(
        collection(vec![entry(
            "archive",
            "/~jdoe/+archive/ubuntu/ppa",
            json!({"name": "ppa", "description": "Line one\nline two", "enabled": true}),
        )]),
        &base,
    );
    let teams_body = rebase(
        collection(vec![person_json("jdoe-team", "Jane's Team", true)]),
        &base,
    );
    let _p1 = mock_get(
        &mut server,
        "/~jdoe",
        Matcher::Missing,
        &rebase(person_json("jdoe", "Jane Doe", false), &base),
    )
    .await;
    let _p2 = mock_get(&mut server, "/people", op("find"), &people_body).await;
    let _p3 = mock_get(
        &mut server,
        "/~ubuntu-dev/members_details",
        Matcher::Missing,
        &members_body,
    )
    .await;
    let _p4 = mock_get(&mut server, "/bugs", op("searchTasks"), &bugs_body).await;
    let _p5 = mock_get(&mut server, "/~jdoe/ppas", Matcher::Missing, &ppas_body).await;
    let _p6 = mock_get(&mut server, "/~jdoe", op("getOwnedTeams"), &teams_body).await;

    // Package mocks
    let sources_body = rebase(collection(vec![source_json("hello", "2.10-3", 1)]), &base);
    let ppa_sources_body = rebase(
        collection(vec![source_json("hello", "2.10-3ppa1", 2)]),
        &base,
    );
    let _pk1 = mock_get(
        &mut server,
        "/ubuntu/+archive/primary",
        op("getPublishedSources"),
        &sources_body,
    )
    .await;
    let _pk2 = mock_get(
        &mut server,
        "/~jdoe/+archive/ubuntu/ppa",
        op("getPublishedSources"),
        &ppa_sources_body,
    )
    .await;

    // Project mocks
    let project_body = rebase(
        entry(
            "project",
            "/launchpad",
            json!({
                "name": "launchpad",
                "display_name": "Launchpad",
                "summary": "Collaboration platform.",
                "homepage_url": "https://launchpad.net/",
            }),
        ),
        &base,
    );
    let active_ms_body = rebase(
        collection(vec![entry(
            "milestone",
            "/launchpad/+milestone/2.0",
            json!({"name": "2.0", "title": "Launchpad 2.0", "is_active": true, "date_targeted": "2025-06-01"}),
        )]),
        &base,
    );
    let proj_series_body = rebase(
        collection(vec![entry(
            "project_series",
            "/launchpad/trunk",
            json!({"name": "trunk", "title": "Main series", "status": "Active Development"}),
        )]),
        &base,
    );
    let releases_body = rebase(
        collection(vec![entry(
            "project_release",
            "/launchpad/trunk/1.0",
            json!({"version": "1.0", "date_released": "2025-06-02T10:00:00+00:00", "release_notes": "First release."}),
        )]),
        &base,
    );
    let _pj1 = mock_get(&mut server, "/launchpad", Matcher::Missing, &project_body).await;
    let _pj2 = mock_get(
        &mut server,
        "/projects",
        op("search"),
        &rebase(collection(vec![project_body.clone()]), &base),
    )
    .await;
    let _pj3 = mock_get(
        &mut server,
        "/launchpad/active_milestones",
        Matcher::Missing,
        &active_ms_body,
    )
    .await;
    let _pj4 = mock_get(
        &mut server,
        "/launchpad/series",
        Matcher::Missing,
        &proj_series_body,
    )
    .await;
    let _pj5 = mock_get(
        &mut server,
        "/launchpad/trunk/releases",
        Matcher::Missing,
        &releases_body,
    )
    .await;

    // CVE mocks
    let cve_one = entry(
        "cve",
        "/bugs/cve/2024-1234",
        json!({
            "sequence": "2024-1234",
            "title": "Curl overflow",
            "status": "Entry",
            "description": "Buffer overflow in curl.",
        }),
    );
    let cve_two = entry(
        "cve",
        "/bugs/cve/2024-9999",
        json!({
            "sequence": "2024-9999",
            "title": "Other bug",
            "status": "Entry",
            "description": "Other issue.",
        }),
    );
    let _c1 = mock_get(
        &mut server,
        "/bugs/cve/2024-1234",
        Matcher::Missing,
        &rebase(cve_one.clone(), &base),
    )
    .await;
    let _c2 = mock_get(
        &mut server,
        "/bugs/cve",
        op("advancedSearch"),
        &rebase(collection(vec![cve_one.clone(), cve_two]), &base),
    )
    .await;
    let _c3 = mock_get(
        &mut server,
        "/bugs/1/cves",
        Matcher::Missing,
        &rebase(collection(vec![cve_one]), &base),
    )
    .await;

    // Spec mocks
    let spec_one = entry(
        "specification",
        "/launchpad/+spec/a-spec",
        json!({
            "name": "a-spec",
            "title": "Spec a-spec",
            "summary": "Make things better.",
            "priority": "High",
            "lifecycle_status": "Started",
            "implementation_status": "Good progress",
            "definition_status": "Approved",
        }),
    );
    let spec_old = entry(
        "specification",
        "/launchpad/+spec/old-spec",
        json!({
            "name": "old-spec",
            "title": "Old spec",
            "summary": "Legacy work.",
            "priority": "Low",
            "lifecycle_status": "Complete",
            "implementation_status": "Implemented",
            "definition_status": "Approved",
        }),
    );
    let _s1 = mock_get(
        &mut server,
        "/launchpad/+spec/a-spec",
        Matcher::Missing,
        &rebase(spec_one.clone(), &base),
    )
    .await;
    let _s2 = mock_get(
        &mut server,
        "/launchpad/valid_specifications",
        Matcher::Missing,
        &rebase(collection(vec![spec_one.clone()]), &base),
    )
    .await;
    let _s3 = mock_get(
        &mut server,
        "/launchpad/all_specifications",
        Matcher::Missing,
        &rebase(collection(vec![spec_one, spec_old]), &base),
    )
    .await;

    // Question mocks
    let q_one = entry(
        "question",
        "/ubuntu/+question/7",
        json!({
            "id": 7,
            "title": "Install help",
            "description": "How do I do this?",
            "status": "Open",
        }),
    );
    let q_two = entry(
        "question",
        "/ubuntu/+question/8",
        json!({"id": 8, "title": "Boot help", "status": "Open"}),
    );
    let q_msgs = rebase(
        collection(vec![
            entry(
                "question_message",
                "/ubuntu/+question/7/messages/1",
                json!({
                    "index": 1,
                    "content": "Try this.",
                    "action": "Answer",
                    "owner_link": format!("{LP}/~helper"),
                    "date_created": "2025-01-01T13:00:00+00:00",
                }),
            ),
            entry(
                "question_message",
                "/ubuntu/+question/7/messages/2",
                json!({
                    "index": 2,
                    "content": "Thanks!",
                    "action": null,
                    "owner_link": format!("{LP}/~jdoe"),
                    "date_created": "2025-01-01T14:00:00+00:00",
                }),
            ),
        ]),
        &base,
    );
    let _q1 = mock_get(
        &mut server,
        "/questions",
        op("getByID"),
        &rebase(q_one.clone(), &base),
    )
    .await;
    let _q2 = mock_get(
        &mut server,
        "/ubuntu",
        op("searchQuestions"),
        &rebase(collection(vec![q_one, q_two]), &base),
    )
    .await;
    let _q3 = mock_get(
        &mut server,
        "/ubuntu/+question/7/messages",
        Matcher::Missing,
        &q_msgs,
    )
    .await;

    // Translation mocks
    let tr_queue = rebase(
        collection(vec![entry(
            "translation_import_queue_entry",
            "/+imports/1",
            json!({
                "path": "po/hello.pot",
                "status": "Needs Review",
                "date_created": "2025-03-04T05:06:07+00:00",
            }),
        )]),
        &base,
    );
    let tr_templates = rebase(
        collection(vec![entry(
            "translation_template",
            "/ubuntu/noble/+source/hello/+pots/hello",
            json!({
                "name": "hello",
                "path": "po/hello.pot",
                "priority": 100,
                "is_current": true,
            }),
        )]),
        &base,
    );
    let _t1 = mock_get(
        &mut server,
        "/ubuntu/noble",
        op("getTranslationImportQueueEntries"),
        &tr_queue,
    )
    .await;
    let _t2 = mock_get(
        &mut server,
        "/ubuntu/noble",
        op("getTranslationTemplates"),
        &tr_templates,
    )
    .await;

    // Unused helper keeps series fixture builder covered if needed.
    let _ = series_json("noble", "24.04", "Supported");

    // 18 remaining read_domains TSV snapshots
    let tsv_cases: &[(&str, &[&str])] = &[
        ("person_search_tsv", &["person", "search", "-q", "jane"]),
        (
            "person_members_tsv",
            &["person", "members", "-t", "ubuntu-dev"],
        ),
        ("person_bugs_tsv", &["person", "bugs", "-n", "jdoe"]),
        ("person_ppas_tsv", &["person", "ppas", "-n", "jdoe"]),
        (
            "person_owned_teams_tsv",
            &["person", "owned-teams", "-n", "jdoe"],
        ),
        (
            "package_search_tsv",
            &["package", "search", "-s", "noble", "-n", "hello"],
        ),
        (
            "package_ppa_sources_tsv",
            &["package", "ppa-sources", "-o", "jdoe", "-p", "ppa"],
        ),
        (
            "project_search_tsv",
            &["project", "search", "-q", "launchpad"],
        ),
        (
            "project_milestones_active_tsv",
            &["project", "milestones", "-p", "launchpad", "--active"],
        ),
        (
            "project_list_series_tsv",
            &["project", "list-series", "-p", "launchpad"],
        ),
        (
            "project_series_releases_tsv",
            &[
                "project",
                "series-releases",
                "-p",
                "launchpad",
                "-s",
                "trunk",
            ],
        ),
        ("cve_search_tsv", &["cve", "search"]),
        ("cve_bug_cves_tsv", &["cve", "bug-cves", "-b", "1"]),
        ("spec_list_tsv", &["spec", "list", "-t", "launchpad"]),
        (
            "spec_list_all_tsv",
            &["spec", "list", "-t", "launchpad", "--all"],
        ),
        (
            "question_search_tsv",
            &["question", "search", "-t", "ubuntu"],
        ),
        (
            "translation_queue_tsv",
            &["translation", "queue", "-s", "noble"],
        ),
        (
            "translation_templates_tsv",
            &["translation", "templates", "-s", "noble"],
        ),
    ];
    for (snap_name, args) in tsv_cases {
        let report = run_read_domain(&server, args).await;
        snap_tsv(snap_name, &report);
    }

    // Representative plain/rich read_domains snapshots
    let human_cases: &[(&str, Option<&str>, Option<&str>)] = &[
        ("person show -n jdoe", None, Some("person_show_rich")),
        (
            "person search -q jane",
            Some("person_search_plain"),
            Some("person_search_rich"),
        ),
        (
            "package search -s noble -n hello",
            Some("package_search_plain"),
            Some("package_search_rich"),
        ),
        (
            "project show -n launchpad",
            Some("project_show_plain"),
            Some("project_show_rich"),
        ),
        (
            "cve show -s 2024-1234",
            Some("cve_show_plain"),
            Some("cve_show_rich"),
        ),
        (
            "cve search",
            Some("cve_search_plain"),
            Some("cve_search_rich"),
        ),
        (
            "spec show -t launchpad -n a-spec",
            None,
            Some("spec_show_rich"),
        ),
        (
            "spec list -t launchpad",
            Some("spec_list_plain"),
            Some("spec_list_rich"),
        ),
        (
            "question show -q 7",
            Some("question_show_plain"),
            Some("question_show_rich"),
        ),
        (
            "question messages -t ubuntu -q 7",
            None,
            Some("question_messages_rich"),
        ),
        (
            "translation queue -s noble",
            Some("translation_queue_plain"),
            Some("translation_queue_rich"),
        ),
    ];
    for (cmdline, plain_snap, rich_snap) in human_cases {
        let args: Vec<&str> = cmdline.split_whitespace().collect();
        let report = run_read_domain(&server, &args).await;
        if let Some(name) = plain_snap {
            snap_plain(name, &report);
        }
        if let Some(name) = rich_snap {
            snap_rich(name, &report);
        }
    }
}

#[tokio::test]
async fn package_download_rich_and_partial_snapshots() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let client = mock_read_client(&server, false);
    let pub_json = rebase(source_json("hello", "2.10-3", 1), &base);
    let _search = mock_get(
        &mut server,
        "/ubuntu/+archive/primary",
        op("getPublishedSources"),
        &collection(vec![pub_json]),
    )
    .await;
    let _urls = mock_get(
        &mut server,
        "/ubuntu/+archive/primary/+sourcepub/1",
        op("sourceFileUrls"),
        &json!([
            format!("{base}/+files/hello_2.10-3.dsc"),
            format!("{base}/+files/hello_2.10.orig.tar.gz"),
        ]),
    )
    .await;
    let _f1 = server
        .mock("GET", "/+files/hello_2.10-3.dsc")
        .with_status(200)
        .with_body("dsc")
        .create_async()
        .await;
    let _f2 = server
        .mock("GET", "/+files/hello_2.10.orig.tar.gz")
        .with_status(200)
        .with_body("orig")
        .create_async()
        .await;

    let ok_dir = tempfile::tempdir().unwrap();
    let req_ok = SourceDownloadRequest {
        name: "hello".into(),
        version: None,
        series: "noble".into(),
        distro: "ubuntu".into(),
        output_dir: ok_dir.path().join("out"),
    };
    let ok_dl = package_download(&client, &req_ok, |_| Ok(()))
        .await
        .unwrap();
    let ok_rich = redact_text(
        render(&package_download_report(&ok_dl), OutputMode::Rich),
        &base,
        Some(ok_dir.path()),
    );
    insta::assert_snapshot!("package_download_success_rich", ok_rich);

    drop(_f2);
    let _f2_404 = server
        .mock("GET", "/+files/hello_2.10.orig.tar.gz")
        .with_status(404)
        .with_body("Missing")
        .create_async()
        .await;
    let partial_dir = tempfile::tempdir().unwrap();
    let req_partial = SourceDownloadRequest {
        name: "hello".into(),
        version: None,
        series: "noble".into(),
        distro: "ubuntu".into(),
        output_dir: partial_dir.path().join("out"),
    };
    let partial_dl = package_download(&client, &req_partial, |_| Ok(()))
        .await
        .unwrap();
    let partial_report = package_download_report(&partial_dl);
    let partial_plain = redact_text(
        render(&partial_report, OutputMode::Plain),
        &base,
        Some(partial_dir.path()),
    );
    assert_plain_invariants(&partial_plain);
    insta::assert_snapshot!("package_download_partial_plain", partial_plain);
    let partial_rich = redact_text(
        render(&partial_report, OutputMode::Rich),
        &base,
        Some(partial_dir.path()),
    );
    insta::assert_snapshot!("package_download_partial_rich", partial_rich);
}

// ---------------------------------------------------------------------------
// Bug mutations and partial-failure plain/rich snapshots
// ---------------------------------------------------------------------------

fn bug_task_json(base: &str, series: Option<&str>, status: &str) -> Value {
    let path = match series {
        Some(s) => format!("ubuntu/{s}/+source/curl"),
        None => "ubuntu/+source/curl".to_string(),
    };
    let display = match series {
        Some(s) => format!("curl (Ubuntu {s})"),
        None => "curl (Ubuntu)".to_string(),
    };
    json!({
        "self_link": format!("{base}/{path}/+bug/42"),
        "web_link": format!("https://bugs.launchpad.net/{path}/+bug/42"),
        "resource_type_link": format!("{base}/#bug_task"),
        "http_etag": "\"task-etag\"",
        "bug_link": format!("{base}/bugs/42"),
        "bug_target_display_name": display,
        "bug_target_name": display,
        "target_link": format!("{LP}/{path}"),
        "status": status,
        "importance": "Medium",
    })
}

async fn run_bug_cmd(server: &mockito::Server, argv: &[&str]) -> BugOutcome {
    let cli = Cli::try_parse_from(argv).unwrap();
    validate_cli(&cli).unwrap();
    let Command::Bug(cmd) = cli.command else {
        panic!("not a bug command");
    };
    let client = mock_read_client(server, true);
    bug_outcome(&client, cmd).await.unwrap()
}

fn snap_bug_outcome(name_prefix: &str, outcome: &BugOutcome, base: &str) {
    for (suffix, mode) in [("plain", OutputMode::Plain), ("rich", OutputMode::Rich)] {
        let out = Output::fixed(mode, false, 100);
        let combined = format!(
            "{}{}",
            outcome.render(&out, false),
            outcome.render_warnings(&out)
        );
        let clean = redact_text(combined, base, None);
        if mode == OutputMode::Plain {
            assert_plain_invariants(&clean);
        } else {
            assert!(!clean.contains('\u{1b}'));
        }
        assert!(!clean.contains("127.0.0.1:"), "{clean}");
        insta::assert_snapshot!(format!("{name_prefix}_{suffix}"), clean);
    }
}

#[tokio::test]
async fn bug_mutation_and_partial_failure_snapshots() {
    // 1. add-comment
    {
        let mut server = isolated_mock_server().await;
        let base = server.url();
        let msg = json!({
            "self_link": format!("{base}/bugs/42/comments/3"),
            "subject": "Re: curl crashes",
            "content": "Thanks!",
            "owner_link": format!("{base}/~commenter"),
            "date_created": "2026-01-03T00:00:00+00:00",
        });
        let _post = server
            .mock("POST", "/bugs/42")
            .match_body(op("newMessage"))
            .with_status(201)
            .with_header("location", &format!("{base}/bugs/42/comments/3"))
            .create_async()
            .await;
        let _read = mock_get(&mut server, "/bugs/42/comments/3", Matcher::Missing, &msg).await;
        let outcome = run_bug_cmd(
            &server,
            &["lpcli", "bug", "add-comment", "-b", "42", "-m", "Thanks!"],
        )
        .await;
        snap_bug_outcome("bug_add_comment", &outcome, &base);
    }

    // 2. set-status (success) and set-status (partial)
    {
        let mut server = isolated_mock_server().await;
        let base = server.url();
        let t_base = bug_task_json(&base, None, "New");
        let t_noble = bug_task_json(&base, Some("noble"), "New");
        let _tasks = mock_get(
            &mut server,
            "/bugs/42/bug_tasks",
            Matcher::Missing,
            &collection(vec![t_base, t_noble]),
        )
        .await;
        let _p_base = server
            .mock("PATCH", "/ubuntu/+source/curl/+bug/42")
            .with_status(200)
            .with_body(bug_task_json(&base, None, "Triaged").to_string())
            .create_async()
            .await;
        let _p_noble = server
            .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
            .with_status(200)
            .with_body(bug_task_json(&base, Some("noble"), "Triaged").to_string())
            .create_async()
            .await;

        let ok = run_bug_cmd(
            &server,
            &[
                "lpcli",
                "bug",
                "set-status",
                "-b",
                "42",
                "-t",
                "curl",
                "--series",
                "noble",
                "-s",
                "triaged",
            ],
        )
        .await;
        snap_bug_outcome("bug_set_status", &ok, &base);

        drop(_p_noble);
        let _p_noble_err = server
            .mock("PATCH", "/ubuntu/noble/+source/curl/+bug/42")
            .with_status(403)
            .with_body("not allowed oauth_token=abc123")
            .create_async()
            .await;
        let partial = run_bug_cmd(
            &server,
            &[
                "lpcli",
                "bug",
                "set-status",
                "-b",
                "42",
                "--all-targets",
                "--all-series",
                "-s",
                "triaged",
            ],
        )
        .await;
        assert_eq!(partial.error.as_ref().unwrap().exit_code(), 7);
        snap_bug_outcome("bug_set_status_partial", &partial, &base);
    }

    // 3. add-task (partial)
    {
        let mut server = isolated_mock_server().await;
        let base = server.url();
        let noble = bug_task_json(&base, Some("noble"), "New");
        let _p1 = server
            .mock("POST", "/bugs/42")
            .match_body(Matcher::UrlEncoded(
                "target".into(),
                format!("{base}/ubuntu/noble/+source/curl"),
            ))
            .with_status(201)
            .with_header(
                "location",
                &format!("{base}/ubuntu/noble/+source/curl/+bug/42"),
            )
            .create_async()
            .await;
        let _p2 = server
            .mock("POST", "/bugs/42")
            .match_body(Matcher::UrlEncoded(
                "target".into(),
                format!("{base}/ubuntu/jammy/+source/curl"),
            ))
            .with_status(400)
            .with_body("Already requested for Jammy")
            .create_async()
            .await;
        let _g = mock_get(
            &mut server,
            "/ubuntu/noble/+source/curl/+bug/42",
            Matcher::Missing,
            &noble,
        )
        .await;
        let outcome = run_bug_cmd(
            &server,
            &[
                "lpcli",
                "bug",
                "add-task",
                "-b",
                "42",
                "-p",
                "curl",
                "--many-series",
                "noble,jammy",
            ],
        )
        .await;
        assert_eq!(outcome.error.as_ref().unwrap().exit_code(), 7);
        snap_bug_outcome("bug_add_task_partial", &outcome, &base);
    }

    // 4. delete-task (partial)
    {
        let mut server = isolated_mock_server().await;
        let base = server.url();
        let _tasks = mock_get(
            &mut server,
            "/bugs/42/bug_tasks",
            Matcher::Missing,
            &collection(vec![
                bug_task_json(&base, None, "New"),
                bug_task_json(&base, Some("noble"), "New"),
            ]),
        )
        .await;
        let _d1 = server
            .mock("DELETE", "/ubuntu/+source/curl/+bug/42")
            .with_status(200)
            .create_async()
            .await;
        let _d2 = server
            .mock("DELETE", "/ubuntu/noble/+source/curl/+bug/42")
            .with_status(403)
            .create_async()
            .await;
        let outcome = run_bug_cmd(
            &server,
            &["lpcli", "bug", "delete-task", "-b", "42", "-p", "curl"],
        )
        .await;
        assert_eq!(outcome.error.as_ref().unwrap().exit_code(), 7);
        snap_bug_outcome("bug_delete_task_partial", &outcome, &base);
    }
}

// ---------------------------------------------------------------------------
// Final accessibility, environment, and snapshot-hygiene audit
// ---------------------------------------------------------------------------

#[test]
fn plain_sanitization_strips_all_banned_symbols_and_controls() {
    let sample = "Status: ─ ┌─┐ ✓ OK ✗ Err ● dot ℹ info \u{2139}\u{FE0F}\u{200D} — em – en \u{202E}bidi\u{2028}sep 📎🚀 ☀ ✂ \t\r\n\u{1b}[31mred";
    let cleaned = output::clean_plain(sample);
    assert_plain_invariants(&cleaned);
    assert!(cleaned.contains("OK OK Error Err"), "{cleaned}");
    assert!(!cleaned.contains('●'), "{cleaned}");
    assert!(!cleaned.contains('ℹ'), "{cleaned}");
    assert!(!cleaned.contains('–'), "{cleaned}");
    assert!(!cleaned.contains('—'), "{cleaned}");
    assert!(!cleaned.contains('☀'), "{cleaned}");
    assert!(!cleaned.contains('✂'), "{cleaned}");
    assert!(!cleaned.contains('📎'), "{cleaned}");
    assert!(!cleaned.contains('\u{202E}'), "{cleaned}");
    assert!(!cleaned.contains('\u{2028}'), "{cleaned}");
    assert!(!cleaned.contains('\u{FE0F}'), "{cleaned}");
    assert!(!cleaned.contains('\u{200D}'), "{cleaned}");

    let dl_err = lpcli::download::DownloadOutcome::Failed {
        filename: "curl.dsc".into(),
        error: "HTTP 404 — ✓ ✗ ● ℹ – 📎 ✖\u{1b}[31m".into(),
    };
    let line = lpcli::download::format_plain_outcome(&dl_err);
    assert_plain_invariants(&line);
    assert!(!line.contains('●'), "{line}");
    assert!(!line.contains('ℹ'), "{line}");
    assert!(!line.contains('✖'), "{line}");
}

#[test]
fn accessibility_env_modes_end_to_end() {
    let settings_default = config::AppConfig::default();
    let settings_accessible = config::AppConfig {
        display: config::DisplayConfig {
            accessible: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let cli = Cli::try_parse_from(["lpcli", "bug", "show", "--bug-id", "42"]).unwrap();
    let env_dumb = output::Environment {
        term: Some("dumb".into()),
        ..Default::default()
    };
    let env_lpcli_plain = output::Environment {
        format: Some("plain".into()),
        ..Default::default()
    };
    let env_no_color = output::Environment {
        no_color: Some("1".into()),
        clicolor_force: None,
        term: Some("xterm-256color".into()),
        ..Default::default()
    };

    let report = Report {
        blocks: vec![
            Block::Heading("Bug #42 — Summary".into()),
            Block::Status(Level::Ok, vec![Span::Text("Status ✓ ready ● ℹ".into())]),
            Block::Field("Assignee", "jdoe".into()),
            Block::Table {
                records: Records {
                    columns: vec![
                        Column::new("Target", "target"),
                        Column::new("Series", "series"),
                        Column::new("Status", "status"),
                    ],
                    rows: vec![vec![
                        Some("curl".into()),
                        Some("noble".into()),
                        Some("Confirmed".into()),
                    ]],
                },
                noun: Noun {
                    singular: "task",
                    plural: "tasks",
                },
                empty: "None.".into(),
                hint: None,
            },
        ],
        json: Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };

    for (label, out) in [
        (
            "TERM=dumb",
            resolve_command_output(&cli, &settings_default, &env_dumb, (true, true)).unwrap(),
        ),
        (
            "LPCLI_FORMAT=plain",
            resolve_command_output(&cli, &settings_default, &env_lpcli_plain, (true, true))
                .unwrap(),
        ),
        (
            "display.accessible=true",
            resolve_command_output(
                &cli,
                &settings_accessible,
                &output::Environment::default(),
                (true, true),
            )
            .unwrap(),
        ),
    ] {
        assert_eq!(out.mode(), OutputMode::Plain, "{label}");
        assert!(!out.stdout_style().color, "{label}");
        let rendered = out.render(&report);
        assert_plain_invariants(&rendered);
        let err = out.render_error(&LpError::NotAuthenticated);
        assert_plain_invariants(&err);
        assert!(err.starts_with("Error: "), "{label}: {err}");
    }

    let no_color_out =
        resolve_command_output(&cli, &settings_default, &env_no_color, (true, true)).unwrap();
    assert_eq!(no_color_out.mode(), OutputMode::Rich);
    assert!(!no_color_out.stdout_style().color);
    let no_color_rendered = no_color_out.render(&report);
    assert!(
        !no_color_rendered.contains('\u{1b}'),
        "{no_color_rendered:?}"
    );
}

#[test]
fn all_snapshots_pass_accessibility_and_redaction_audit() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dirs = [
        manifest_dir.join("src/bin/lpcli/snapshots"),
        manifest_dir.join("src/bin/lpcli/tests/snapshots"),
    ];
    let mut checked = 0usize;

    for dir in &dirs {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read snapshot dir {}: {e}", dir.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "snap"))
            .collect();
        entries.sort();

        for path in entries {
            checked += 1;
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let body = content
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---\n"))
                .map(|(_, body)| body)
                .unwrap_or(&content);

            assert!(
                !body.contains("127.0.0.1:"),
                "ephemeral mock-server address leaked into {}",
                path.display()
            );
            for secret_key in [
                "oauth_token_secret=",
                "oauth_signature=",
                "refresh_token_secret=",
                "oauth_token=",
                "oauth_verifier=",
                "consumer_secret=",
                "oauth_consumer_key=",
                "access_token=",
                "refresh_token=",
                "\"oauth_token_secret\":",
                "\"oauth_token\":",
                "\"oauth_signature\":",
                "\"refresh_token_secret\":",
                "\"refresh_token\":",
                "\"access_token\":",
            ] {
                for (idx, _) in body.match_indices(secret_key) {
                    let tail = body[idx + secret_key.len()..].trim_start();
                    assert!(
                        tail.starts_with("[redacted]") || tail.starts_with("\"[redacted]\""),
                        "unredacted credential marker {secret_key:?} in {}",
                        path.display()
                    );
                }
            }
            assert_eq!(
                output::redact_secrets(body),
                body,
                "unredacted credential or Authorization header in {}",
                path.display()
            );
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_plain.snap"))
            {
                assert_plain_invariants(body);
            }
        }
    }

    assert!(
        checked > 200,
        "expected full snapshot set, checked {checked}"
    );
}
