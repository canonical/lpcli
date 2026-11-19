//! Phase 1 CLI tests for git commands.
//!
//! Every git read runs against a mock Launchpad and is checked for:
//!
//! - fidelity: the JSON report equals the Launchpad body exactly (entries),
//!   or Launchpad's collection shape with unmodified entries (lists);
//! - an insta JSON snapshot, with server URLs and timestamps redacted;
//! - `plain` and `tsv` invariants (and `tsv` support only for lists).
//!
//! `git comments` is an lpcli-defined object whose `all_comments` and
//! `inline_comments` members are Launchpad's responses, unchanged.

#[allow(unused_imports)]
use super::*;
use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";
const REPO: &str = "~owner/project/+git/repo";

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
            ".**.date_last_modified" => "[timestamp]",
            ".**.date" => "[timestamp]",
        })
    };
}

fn render(report: &Report, mode: OutputMode) -> String {
    Output::fixed(mode, false, 100).render(report)
}

/// JSON parses back to the report's value, `plain` has no decoration, and
/// `rich` has no escapes without color.
fn assert_modes(report: &Report) {
    let json: Value = serde_json::from_str(&render(report, OutputMode::Json))
        .expect("json output must always parse");
    assert_eq!(json, report.json);
    assert_plain_invariants(&render(report, OutputMode::Plain));
    assert!(!render(report, OutputMode::Rich).contains('\u{1b}'));
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

fn repository_json(name: &str) -> Value {
    json!({
        "self_link": format!("{LP}/~owner/project/+git/{name}"),
        "web_link": format!("https://code.launchpad.net/~owner/project/+git/{name}"),
        "resource_type_link": format!("{LP}/#git_repository"),
        "http_etag": "\"etag-repo\"",
        "name": name,
        "unique_name": format!("~owner/project/+git/{name}"),
        "description": "A test\trepository",
        "owner_default": false,
        "target_default": true,
        "repository_type": "Hosted",
        "information_type": "Public",
        "private": false,
        "owner_link": format!("{LP}/~owner"),
        "target_link": format!("{LP}/project"),
        "default_branch": "refs/heads/main",
        "git_https_url": format!("https://git.launchpad.net/~owner/project/+git/{name}"),
        "date_created": "2025-01-02T03:04:05.678901+00:00",
        "date_last_modified": "2026-02-03T04:05:06.789012+00:00",
        "loose_object_count": null,
        "pack_count": null,
    })
}

fn ref_json(path: &str, sha1: &str) -> Value {
    json!({
        "self_link": format!("{LP}/{REPO}/+ref/{path}"),
        "resource_type_link": format!("{LP}/#git_ref"),
        "http_etag": "\"etag-ref\"",
        "path": path,
        "display_name": path.trim_start_matches("refs/heads/"),
        "commit_sha1": sha1,
        "commit_message": "Initial\ncommit",
        "repository_link": format!("{LP}/{REPO}"),
        "web_link": null,
    })
}

fn proposal_json(id: u64, status: &str) -> Value {
    json!({
        "self_link": format!("{LP}/{REPO}/+merge/{id}"),
        "web_link": format!("https://code.launchpad.net/{REPO}/+merge/{id}"),
        "resource_type_link": format!("{LP}/#branch_merge_proposal"),
        "http_etag": "\"etag-mp\"",
        "queue_status": status,
        "commit_message": null,
        "description": "Fix things",
        "source_git_repository_link": format!("{LP}/{REPO}"),
        "source_git_path": "refs/heads/fix",
        "target_git_repository_link": format!("{LP}/~owner/project/+git/main"),
        "target_git_path": "refs/heads/main",
        "registrant_link": format!("{LP}/~owner"),
        "all_comments_collection_link": format!("{LP}/{REPO}/+merge/{id}/all_comments"),
        "preview_diff_link": format!("{LP}/{REPO}/+merge/{id}/+preview-diff/77"),
        "preview_diffs_collection_link": format!("{LP}/{REPO}/+merge/{id}/preview_diffs"),
        "date_created": "2026-03-04T05:06:07.890123+00:00",
        "date_last_modified": null,
        "reviewed_revid": null,
    })
}

fn preview_diff_json(id: u64) -> Value {
    json!({
        "self_link": format!("{LP}/{REPO}/+merge/1/+preview-diff/{id}"),
        "resource_type_link": format!("{LP}/#preview_diff"),
        "http_etag": "\"etag-diff\"",
        "id": id,
        "title": null,
        "diff_text_link": format!("{LP}/{REPO}/+merge/1/+preview-diff/{id}/diff_text"),
        "diff_lines_count": 12,
        "added_lines_count": 3,
        "removed_lines_count": 1,
        "diffstat": {"lib/model.py": [3, 1]},
        "source_revision_id": "0123456789abcdef0123456789abcdef01234567",
        "target_revision_id": "fedcba9876543210fedcba9876543210fedcba98",
        "conflicts": null,
        "date_created": "2026-03-05T06:07:08.901234+00:00",
    })
}

fn review_comment_json(id: u64, vote: Option<&str>) -> Value {
    json!({
        "self_link": format!("{LP}/{REPO}/+merge/1/comments/{id}"),
        "web_link": format!("https://code.launchpad.net/{REPO}/+merge/1/comments/{id}"),
        "resource_type_link": format!("{LP}/#code_review_comment"),
        "http_etag": "\"etag-comment\"",
        "id": id,
        "title": format!("Re: comment {id}"),
        "content": "Looks good,\nbut see inline.",
        "vote": vote,
        "vote_tag": null,
        "author_link": format!("{LP}/~reviewer"),
        "date_created": "2026-03-06T07:08:09.012345+00:00",
        "message_body": "Looks good,\nbut see inline.",
    })
}

fn git_command(argv: &[&str]) -> GitCommand {
    let mut full = vec!["lpcli", "git"];
    full.extend(argv);
    match Cli::try_parse_from(full).unwrap().command {
        Command::Git(command) => command,
        other => panic!("unexpected command: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Supported modes
// ---------------------------------------------------------------------------

#[test]
fn every_git_read_supports_json_and_lists_support_tsv() {
    let reads: [(&[&str], bool); 8] = [
        (&["show", "--path", REPO], false),
        (&["default", "--target", "project"], false),
        (&["list-person-repos", "--name", "owner"], true),
        (&["list", "--target", "project"], true),
        (&["refs", "--path", REPO], true),
        (&["proposals", "--path", REPO], true),
        (&["comments", "--path", REPO, "--id", "1"], false),
        (&["diffs", "--path", REPO, "--id", "1"], true),
    ];
    for (argv, list) in reads {
        let command = git_command(argv);
        assert!(git_supports_json(&command), "{argv:?}");
        assert_eq!(git_supports_tsv(&command), list, "{argv:?}");
        for (format, supported) in [("json", true), ("tsv", list)] {
            let mut full = vec!["lpcli", "--format", format, "git"];
            full.extend(argv);
            let cli = Cli::try_parse_from(&full).unwrap();
            assert_eq!(validate_cli(&cli).is_ok(), supported, "{full:?}");
        }
        let mut full = vec!["lpcli", "--format", "json", "--fields", "self_link", "git"];
        full.extend(argv);
        validate_cli(&Cli::try_parse_from(&full).unwrap()).unwrap();
    }
}

#[test]
fn git_reads_reject_quiet() {
    let cli = Cli::try_parse_from(["lpcli", "--quiet", "git", "show", "--path", REPO]).unwrap();
    assert!(validate_cli(&cli).is_err());
}

// ---------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_show_prints_the_launchpad_entry() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = rebase(repository_json("repo"), &base);
    let mock = server
        .mock("GET", format!("/{REPO}").as_str())
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(&client, git_command(&["show", "--path", REPO]))
        .await
        .unwrap();
    assert_eq!(report.json, body, "JSON must be the Launchpad entry");
    assert_modes(&report);
    assert_json_report!("git_show_json", stable(&report.json, &base));
    let plain = render(&report, OutputMode::Plain);
    assert!(plain.starts_with("~owner/project/+git/repo\n"), "{plain}");
    assert!(plain.contains("Type: Hosted\n"), "{plain}");
    mock.assert_async().await;
}

#[tokio::test]
async fn git_default_prints_the_launchpad_entry() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = rebase(repository_json("project"), &base);
    let target = format!("{base}/project");
    let mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
            Matcher::UrlEncoded("target".into(), target),
        ]))
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(&client, git_command(&["default", "--target", "project"]))
        .await
        .unwrap();
    assert_eq!(report.json, body);
    assert_modes(&report);
    assert_json_report!("git_default_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn git_default_handles_source_packages_null_and_invalid_targets() {
    use clap::CommandFactory as _;

    let mut server = isolated_mock_server().await;
    let base = server.url();
    let pkg_body = rebase(repository_json("unbound"), &base);

    let pkg_mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/ubuntu/+source/unbound")),
        ]))
        .with_body(pkg_body.to_string())
        .create_async()
        .await;
    let null_mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/unbound")),
        ]))
        .with_status(200)
        .with_body("null")
        .create_async()
        .await;
    let distro_mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/ubuntu")),
        ]))
        .with_status(400)
        .with_body("target: (None, None)")
        .create_async()
        .await;
    let series_mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/tenacious")),
        ]))
        .with_status(400)
        .with_body(format!("target: No such object \"{base}/tenacious\"."))
        .create_async()
        .await;

    let client = mock_read_client(&server, false);

    // 1. Valid distribution source package target.
    let ok_report = git_report(
        &client,
        git_command(&["default", "--target", "ubuntu/+source/unbound"]),
    )
    .await
    .unwrap();
    assert_eq!(ok_report.json, pkg_body);
    assert_modes(&ok_report);
    pkg_mock.assert_async().await;

    // 2. Project with no default Git repository configured (200 null) -> exit 4 + structured not_found.
    let err_unbound = git_report(&client, git_command(&["default", "--target", "unbound"]))
        .await
        .unwrap_err();
    assert_eq!(err_unbound.exit_code(), 4, "{err_unbound:?}");
    let unbound_json: Value = serde_json::from_str(
        &Output::fixed(OutputMode::Json, false, 100).render_error(&err_unbound),
    )
    .unwrap();
    assert_eq!(unbound_json["error"]["kind"], "not_found");
    assert_eq!(unbound_json["error"]["http_status"], Value::Null);
    assert!(
        unbound_json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ubuntu/+source/unbound"),
        "{unbound_json}"
    );
    null_mock.assert_async().await;

    // 3. Bare distribution target rejected by Launchpad -> exit 2 + structured usage.
    let err_ubuntu = git_report(&client, git_command(&["default", "--target", "ubuntu"]))
        .await
        .unwrap_err();
    assert_eq!(err_ubuntu.exit_code(), 2, "{err_ubuntu:?}");
    let ubuntu_json: Value = serde_json::from_str(
        &Output::fixed(OutputMode::Json, false, 100).render_error(&err_ubuntu),
    )
    .unwrap();
    assert_eq!(ubuntu_json["error"]["kind"], "usage");
    assert_eq!(ubuntu_json["error"]["http_status"], Value::Null);
    distro_mock.assert_async().await;

    // 4. Unknown target such as a series codename -> exit 4 + structured not_found.
    let err_tenacious = git_report(&client, git_command(&["default", "--target", "tenacious"]))
        .await
        .unwrap_err();
    assert_eq!(err_tenacious.exit_code(), 4, "{err_tenacious:?}");
    let tenacious_json: Value = serde_json::from_str(
        &Output::fixed(OutputMode::Json, false, 100).render_error(&err_tenacious),
    )
    .unwrap();
    assert_eq!(tenacious_json["error"]["kind"], "not_found");
    assert_eq!(tenacious_json["error"]["http_status"], Value::Null);
    series_mock.assert_async().await;

    // 5. CLI help reflects actual Launchpad targets.
    let mut cmd = Cli::command();
    let git_cmd = cmd.find_subcommand_mut("git").unwrap();
    let default_cmd = git_cmd.find_subcommand_mut("default").unwrap();
    let help = default_cmd.render_long_help().to_string();
    assert!(help.contains("ubuntu/+source/"), "{help}");
    assert!(!help.contains("Project or distribution name"), "{help}");
}

#[test]
fn git_fields_select_launchpad_attributes() {
    let repo = Raw::from_value(repository_json("repo")).unwrap();
    let report = git_repository_report(&repo);
    let rendered = Output::fixed(OutputMode::Json, false, 100)
        .with_fields(&["unique_name", "web_link", "missing"])
        .render(&report);
    assert_eq!(
        serde_json::from_str::<Value>(&rendered).unwrap(),
        json!({
            "unique_name": "~owner/project/+git/repo",
            "web_link": "https://code.launchpad.net/~owner/project/+git/repo",
        })
    );
}

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_list_person_repos_follows_pages_into_one_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let first = rebase(repository_json("one"), &base);
    let second = rebase(repository_json("two"), &base);
    let page_one = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getRepositories".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/~owner")),
        ]))
        .with_body(
            json!({
                "total_size": 2,
                "start": 0,
                "entries": [first],
                "next_collection_link": format!("{base}/+git?ws.op=getRepositories&target={base}%2F~owner&ws.start=1&ws.size=1"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let page_two = server
        .mock("GET", "/+git")
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_body(json!({"start": 1, "entries": [second]}).to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(
        &client,
        git_command(&["list-person-repos", "--name", "owner"]),
    )
    .await
    .unwrap();
    assert_eq!(
        report.json,
        collection(vec![first, second]),
        "every page fetched: entries unmodified, no next_collection_link"
    );
    assert_modes(&report);
    let tsv = render(&report, OutputMode::Tsv);
    assert_tsv_invariants(&tsv, 2);
    assert!(tsv.starts_with("unique_name\trepository_type\tdate_last_modified\n"));
    assert_json_report!("git_list_person_repos_json", stable(&report.json, &base));
    page_one.assert_async().await;
    page_two.assert_async().await;
}

#[tokio::test]
async fn git_list_target_prints_the_launchpad_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = collection(vec![
        rebase(repository_json("one"), &base),
        rebase(repository_json("two"), &base),
    ]);
    let mock = server
        .mock("GET", "/+git")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getRepositories".into()),
            Matcher::UrlEncoded("target".into(), format!("{base}/project")),
        ]))
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(&client, git_command(&["list", "--target", "project"]))
        .await
        .unwrap();
    assert_eq!(report.json, body, "JSON must be the Launchpad collection");
    assert_modes(&report);
    let tsv = render(&report, OutputMode::Tsv);
    assert_tsv_invariants(&tsv, 2);
    assert_json_report!("git_list_target_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn git_refs_prints_the_launchpad_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let entries = vec![
        rebase(
            ref_json(
                "refs/heads/main",
                "0123456789abcdef0123456789abcdef01234567",
            ),
            &base,
        ),
        rebase(
            ref_json("refs/tags/v1", "fedcba9876543210fedcba9876543210fedcba98"),
            &base,
        ),
    ];
    let body = collection(entries);
    let mock = server
        .mock("GET", format!("/{REPO}/refs").as_str())
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(&client, git_command(&["refs", "--path", REPO]))
        .await
        .unwrap();
    assert_eq!(report.json, body);
    assert_modes(&report);
    let tsv = render(&report, OutputMode::Tsv);
    assert_tsv_invariants(&tsv, 2);
    assert!(
        tsv.contains("\t0123456789abcdef0123456789abcdef01234567\n"),
        "TSV keeps the full SHA1: {tsv}"
    );
    assert_json_report!("git_refs_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn git_proposals_prints_the_launchpad_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = collection(vec![
        rebase(proposal_json(11, "Needs review"), &base),
        rebase(proposal_json(12, "Approved"), &base),
    ]);
    let mock = server
        .mock("GET", format!("/{REPO}").as_str())
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getMergeProposals".into()),
            Matcher::UrlEncoded("status".into(), "Needs review".into()),
        ]))
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(
        &client,
        git_command(&["proposals", "--path", REPO, "--status", "Needs review"]),
    )
    .await
    .unwrap();
    assert_eq!(report.json, body);
    assert_modes(&report);
    let tsv = render(&report, OutputMode::Tsv);
    assert_tsv_invariants(&tsv, 2);
    assert!(
        tsv.lines()
            .nth(1)
            .unwrap()
            .starts_with("11\tNeeds review\t")
    );
    assert_json_report!("git_proposals_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn git_diffs_prints_the_launchpad_collection() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let mp = server
        .mock("GET", format!("/{REPO}/+merge/1").as_str())
        .with_body(rebase(proposal_json(1, "Needs review"), &base).to_string())
        .create_async()
        .await;
    let body = collection(vec![
        rebase(preview_diff_json(76), &base),
        rebase(preview_diff_json(77), &base),
    ]);
    let diffs = server
        .mock("GET", format!("/{REPO}/+merge/1/preview_diffs").as_str())
        .with_body(body.to_string())
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let report = git_report(
        &client,
        git_command(&["diffs", "--path", REPO, "--id", "1"]),
    )
    .await
    .unwrap();
    assert_eq!(report.json, body);
    assert_modes(&report);
    assert_tsv_invariants(&render(&report, OutputMode::Tsv), 2);
    assert_json_report!("git_diffs_json", stable(&report.json, &base));
    mp.assert_async().await;
    diffs.assert_async().await;
}

#[test]
fn git_empty_lists_keep_the_collection_shape() {
    let empty: Raw<Vec<git::PreviewDiff>> = Raw {
        value: Vec::new(),
        raw: collection(Vec::new()),
    };
    let report = git_diffs_report(&empty);
    assert_eq!(
        render(&report, OutputMode::Plain),
        "No preview diffs found.\n"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&render(&report, OutputMode::Json)).unwrap(),
        json!({"total_size": 0, "start": 0, "entries": []})
    );
    assert_tsv_invariants(&render(&report, OutputMode::Tsv), 0);
}

// ---------------------------------------------------------------------------
// git comments
// ---------------------------------------------------------------------------

const DIFF_TEXT: &str = "\
diff --git a/lib/model.py b/lib/model.py
--- a/lib/model.py
+++ b/lib/model.py
@@ -10,2 +10,3 @@ class Foo:
     def existing(self):
+        return True
";

#[tokio::test]
async fn git_comments_is_an_lpcli_object_of_unmodified_launchpad_responses() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let mp = server
        .mock("GET", format!("/{REPO}/+merge/1").as_str())
        .match_query(Matcher::Missing)
        .with_body(rebase(proposal_json(1, "Needs review"), &base).to_string())
        .expect_at_least(1)
        .create_async()
        .await;
    let comments_body = collection(vec![
        rebase(review_comment_json(5, Some("Approve")), &base),
        rebase(review_comment_json(6, None), &base),
    ]);
    let comments = server
        .mock("GET", format!("/{REPO}/+merge/1/all_comments").as_str())
        .with_body(comments_body.to_string())
        .create_async()
        .await;
    let inline_body = json!([
        {
            "line_number": "6",
            "person": {"name": "reviewer", "self_link": format!("{base}/~reviewer")},
            "text": "Why True?",
            "date": "2026-03-06T07:08:09.012345+00:00",
        },
        {"line_number": "99", "person": format!("{base}/~other"), "text": "Unmapped", "date": null},
    ]);
    let inline = server
        .mock("GET", format!("/{REPO}/+merge/1").as_str())
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getInlineComments".into()),
            Matcher::UrlEncoded("previewdiff_id".into(), "77".into()),
        ]))
        .with_body(inline_body.to_string())
        .create_async()
        .await;
    // The diff text itself is only fetched from code.launchpad.net, so a
    // mock-server `diff_text_link` leaves every location unknown, without a
    // network request. Locations are covered by the unit test below.
    let diff = server
        .mock("GET", format!("/{REPO}/+merge/1/+preview-diff/77").as_str())
        .with_body(rebase(preview_diff_json(77), &base).to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, false);
    let report = git_report(
        &client,
        git_command(&["comments", "--path", REPO, "--id", "1"]),
    )
    .await
    .unwrap();
    assert_eq!(report.json["all_comments"], comments_body);
    assert_eq!(report.json["inline_comments"], inline_body);
    assert_eq!(
        report.json["lpcli"],
        json!({"inline_comment_locations": [null, null]}),
        "lpcli-computed context stays out of Launchpad entries"
    );
    assert_modes(&report);
    let plain = render(&report, OutputMode::Plain);
    assert!(plain.contains("Location: diff line 6\n"), "{plain}");
    assert!(plain.contains("Vote: Approve\n"), "{plain}");
    assert_json_report!("git_comments_json", stable(&report.json, &base));
    mp.assert_async().await;
    comments.assert_async().await;
    inline.assert_async().await;
    diff.assert_async().await;
}

#[test]
fn git_comments_locations_come_from_the_diff_map_under_lpcli() {
    let inline_raw = json!([
        {"line_number": "6", "person": {"name": "reviewer"}, "text": "Why True?", "date": null},
        {"line_number": "99", "person": null, "text": "Unmapped", "date": null},
    ]);
    let comments = GitComments {
        comments: Raw {
            value: Vec::new(),
            raw: collection(Vec::new()),
        },
        inline: Raw::from_value(inline_raw.clone()).unwrap(),
        diff_map: git::parse_diff_line_map(DIFF_TEXT),
    };
    let report = git_comments_report(&comments);
    assert_eq!(report.json["inline_comments"], inline_raw);
    assert_eq!(
        report.json["lpcli"],
        json!({"inline_comment_locations": [
            {"file": "lib/model.py", "new_line": 11, "content": "        return True"},
            null,
        ]})
    );
    let plain = render(&report, OutputMode::Plain);
    assert_plain_invariants(&plain);
    assert!(plain.contains("Location: lib/model.py:11\n"), "{plain}");
    assert!(
        plain.contains("Diff line:         return True\n"),
        "{plain}"
    );
    assert!(plain.contains("Location: diff line 99\n"), "{plain}");
}

#[test]
fn git_comments_without_a_preview_diff_has_empty_inline_comments() {
    let comments = GitComments {
        comments: Raw {
            value: Vec::new(),
            raw: collection(Vec::new()),
        },
        inline: Raw {
            value: Vec::new(),
            raw: json!([]),
        },
        diff_map: Vec::new(),
    };
    let report = git_comments_report(&comments);
    assert_eq!(
        report.json,
        json!({
            "all_comments": {"total_size": 0, "start": 0, "entries": []},
            "inline_comments": [],
            "lpcli": {"inline_comment_locations": []},
        })
    );
    assert_eq!(
        render(&report, OutputMode::Plain),
        "Review comments\nNo review comments.\n"
    );
    // Field selection keeps the lpcli member and collection shape.
    let rendered = Output::fixed(OutputMode::Json, false, 100)
        .with_fields(&["id"])
        .render(&report);
    assert_eq!(
        serde_json::from_str::<Value>(&rendered).unwrap(),
        report.json
    );
}
