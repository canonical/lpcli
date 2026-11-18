//! Phase 1 CLI tests for read_domains commands.
//!
//! Every person, package, project, CVE, spec, question, and translation read
//! runs against a mock Launchpad and is checked for:
//!
//! - fidelity: the JSON report equals the Launchpad body exactly (entries),
//!   or Launchpad's collection shape with unmodified entries (lists);
//! - an insta JSON snapshot, with server URLs and timestamps redacted;
//! - `plain` and `tsv` invariants (and `tsv` support only for lists).
//!
//! `package download` is checked for its lpcli-defined JSON object and its
//! exit status: partial failures are `LpError::Partial` (exit 7), and a
//! download where every file failed is a general error (exit 1).

#[allow(unused_imports)]
use super::*;
use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use lpcli::error::LpError;
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";

/// Replace the production API root in every string, so links in mocked
/// bodies point at the mock server, and pagination links can be followed.
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

/// The JSON report with the mock-server URL replaced by the production API
/// root, so snapshots do not depend on the server's port.
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

/// Snapshot a JSON report with timestamps redacted.
macro_rules! assert_json_report {
    ($name:expr, $value:expr) => {
        insta::assert_json_snapshot!($name, $value, {
            ".**.date_created" => "[timestamp]",
            ".**.date_joined" => "[timestamp]",
            ".**.date_released" => "[timestamp]",
            ".**.date_made_public" => "[timestamp]",
            ".**.date_last_updated" => "[timestamp]",
            ".**.date_published" => "[timestamp]",
        })
    };
}

fn render(report: &Report, mode: OutputMode) -> String {
    Output::fixed(mode, false, 100).render(report)
}

/// Shared checks for every report: JSON parses back to the report's value,
/// `plain` has no decoration, and `rich` has no escapes without color.
fn assert_modes(report: &Report) {
    let json: Value = serde_json::from_str(&render(report, OutputMode::Json))
        .expect("json output must always parse");
    assert_eq!(json, report.json);
    assert_plain_invariants(&render(report, OutputMode::Plain));
    assert!(!render(report, OutputMode::Rich).contains('\u{1b}'));
}

/// Checks for a list report: `tsv` has one line per entry.
fn assert_list(report: &Report) {
    assert_modes(report);
    let entries = report.json["entries"]
        .as_array()
        .expect("lists use Launchpad's collection shape")
        .len();
    assert_eq!(report.records.rows.len(), entries);
    assert_tsv_invariants(&render(report, OutputMode::Tsv), entries);
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
        .match_header("authorization", Matcher::Missing)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

fn op(name: &str) -> Matcher {
    Matcher::UrlEncoded("ws.op".into(), name.into())
}

fn entry(kind: &str, path: &str, fields: Value) -> Value {
    let mut value = json!({
        "self_link": format!("{LP}{path}"),
        "web_link": format!("https://launchpad.net{path}"),
        "resource_type_link": format!("{LP}/#{kind}"),
        "http_etag": "\"etag-1234\"",
    });
    for (key, field) in fields.as_object().into_iter().flatten() {
        value[key] = field.clone();
    }
    value
}

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
            "time_zone": "UTC",
            "date_created": "2005-08-09T19:50:16.647642+00:00",
            "hide_email_addresses": true,
        }),
    )
}

fn collection(entries: Vec<Value>) -> Value {
    json!({"total_size": entries.len(), "start": 0, "entries": entries})
}

fn parsed(args: &[&str]) -> Command {
    let mut argv = vec!["lpcli"];
    argv.extend(args);
    let cli = Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    validate_cli(&cli).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    cli.command
}

/// Run one read command against the mock server and check its JSON is the
/// mocked Launchpad body, unchanged.
async fn run(server: &mockito::Server, args: &[&str]) -> Report {
    let client = mock_read_client(server, false);
    match parsed(args) {
        Command::Person(cmd) => person_report(&client, cmd).await,
        Command::Package(cmd) => package_report(&client, cmd).await,
        Command::Project(cmd) => project_report(&client, cmd).await,
        Command::Cve(cmd) => cve_report(&client, cmd).await,
        Command::Spec(cmd) => spec_report(&client, cmd).await,
        Command::Question(cmd) => question_report(&client, cmd).await,
        Command::Translation(cmd) => translation_report(&client, cmd).await,
        other => panic!("not a read-domain command: {other:?}"),
    }
    .unwrap_or_else(|e| panic!("{args:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Supported output modes
// ---------------------------------------------------------------------------

#[test]
fn every_read_command_supports_json_and_lists_support_tsv() {
    let lists: &[&[&str]] = &[
        &["person", "search", "-q", "jane"],
        &["person", "members", "-t", "ubuntu-dev"],
        &["person", "bugs", "-n", "jdoe"],
        &["person", "ppas", "-n", "jdoe"],
        &["person", "owned-teams", "-n", "jdoe"],
        &["package", "list-series"],
        &["package", "search", "-s", "noble", "-n", "hello"],
        &["package", "ppa-sources", "-o", "jdoe", "-p", "ppa"],
        &["project", "search", "-q", "launchpad"],
        &["project", "milestones", "-p", "launchpad"],
        &["project", "list-series", "-p", "launchpad"],
        &[
            "project",
            "series-releases",
            "-p",
            "launchpad",
            "-s",
            "trunk",
        ],
        &["cve", "search"],
        &["cve", "bug-cves", "-b", "1"],
        &["spec", "list", "-t", "launchpad"],
        &["question", "search", "-t", "ubuntu"],
        &["question", "messages", "-t", "ubuntu", "-q", "1"],
        &["translation", "queue", "-s", "noble"],
        &["translation", "templates", "-s", "noble"],
    ];
    let entries: &[&[&str]] = &[
        &["person", "show", "-n", "jdoe"],
        &["package", "series", "-s", "noble"],
        &["package", "ppa", "-o", "jdoe", "-p", "ppa"],
        &["package", "distro"],
        &["package", "download", "-n", "hello", "-s", "noble"],
        &["project", "show", "-n", "launchpad"],
        &["project", "show-milestone", "-p", "launchpad", "-n", "1.0"],
        &["project", "series-show", "-p", "launchpad", "-s", "trunk"],
        &["cve", "show", "-s", "2024-1234"],
        &["spec", "show", "-t", "launchpad", "-n", "a-spec"],
        &["question", "show", "-q", "1"],
    ];
    for (args, tsv) in lists
        .iter()
        .map(|args| (args, true))
        .chain(entries.iter().map(|args| (args, false)))
    {
        let command = parsed(args);
        assert!(read_domain_supports_json(&command), "{args:?}");
        assert_eq!(read_domain_supports_tsv(&command), tsv, "{args:?}");
        for format in ["json", "tsv"] {
            let mut argv = vec!["lpcli", "--format", format];
            argv.extend(args.iter());
            let cli = Cli::try_parse_from(&argv).unwrap();
            assert_eq!(
                validate_cli(&cli).is_ok(),
                format == "json" || tsv,
                "{argv:?}"
            );
        }
    }
    let other = parsed(&["bug", "show", "-b", "1"]);
    assert!(!read_domain_supports_json(&other));
    assert!(!read_domain_supports_tsv(&other));
}

#[test]
fn read_domain_json_accepts_fields_and_public() {
    for argv in [
        &[
            "lpcli",
            "--public",
            "--format",
            "json",
            "--fields",
            "name,display_name",
            "person",
            "show",
            "-n",
            "jdoe",
        ][..],
        &[
            "lpcli", "--public", "--format", "tsv", "cve", "search", "-k", "curl",
        ],
    ] {
        validate_cli(&Cli::try_parse_from(argv).unwrap()).unwrap();
    }
    let report = entry_report(person_json("jdoe", "Jane", false), "Jane".into(), vec![]);
    let selected = Output::fixed(OutputMode::Json, false, 100)
        .with_fields(&["name", "missing"])
        .render(&report);
    assert_eq!(
        serde_json::from_str::<Value>(&selected).unwrap(),
        json!({"name": "jdoe"})
    );
}

// ---------------------------------------------------------------------------
// person
// ---------------------------------------------------------------------------

#[tokio::test]
async fn person_show_prints_the_launchpad_entry() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = rebase(person_json("jdoe", "Jane Doe", false), &base);
    let mock = mock_get(&mut server, "/~jdoe", Matcher::Missing, &body).await;
    let report = run(&server, &["person", "show", "-n", "jdoe"]).await;
    assert_eq!(report.json, body);
    assert_modes(&report);
    let plain = render(&report, OutputMode::Plain);
    assert!(
        plain.contains("Name: jdoe\nType: Person\nKarma: 42\n"),
        "{plain}"
    );
    insta::assert_snapshot!("person_show_plain", plain);
    assert_json_report!("person_show_json", stable(&report.json, &base));
    mock.assert_async().await;
}

#[tokio::test]
async fn person_lists_print_launchpad_collections() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
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
                "date_joined": "2020-01-02T03:04:05.000000+00:00",
                "date_expires": null,
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
                "date_created": "2026-01-02T03:04:05.678901+00:00",
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
    let mocks = [
        mock_get(&mut server, "/people", op("find"), &people_body).await,
        mock_get(
            &mut server,
            "/~ubuntu-dev/members_details",
            Matcher::Missing,
            &members_body,
        )
        .await,
        mock_get(&mut server, "/bugs", op("searchTasks"), &bugs_body).await,
        mock_get(&mut server, "/~jdoe/ppas", Matcher::Missing, &ppas_body).await,
        mock_get(&mut server, "/~jdoe", op("getOwnedTeams"), &teams_body).await,
    ];
    for (name, args, body) in [
        (
            "person_search",
            &["person", "search", "-q", "jane"][..],
            &people_body,
        ),
        (
            "person_members",
            &["person", "members", "-t", "ubuntu-dev"],
            &members_body,
        ),
        ("person_bugs", &["person", "bugs", "-n", "jdoe"], &bugs_body),
        ("person_ppas", &["person", "ppas", "-n", "jdoe"], &ppas_body),
        (
            "person_owned_teams",
            &["person", "owned-teams", "-n", "jdoe"],
            &teams_body,
        ),
    ] {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_list(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    let report = run(&server, &["person", "search", "-q", "jane"]).await;
    assert_eq!(
        render(&report, OutputMode::Tsv),
        "name\tdisplay_name\ttype\njdoe\tJane Doe\tPerson\nubuntu-dev\tUbuntu Development Team\tTeam\n"
    );
    let report = run(&server, &["person", "members", "-t", "ubuntu-dev"]).await;
    assert_eq!(
        render(&report, OutputMode::Tsv).lines().nth(1),
        Some("jdoe\tApproved\t2020-01-02")
    );
    let report = run(&server, &["person", "bugs", "-n", "jdoe"]).await;
    assert_eq!(
        render(&report, OutputMode::Tsv).lines().nth(1),
        Some("7\tCrash\\twith tab\tNew\tUndecided")
    );
    for mock in mocks {
        mock.expect_at_least(1).assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// package
// ---------------------------------------------------------------------------

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
            "date_created": "2023-10-01T00:00:00+00:00",
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
            "date_superseded": null,
            "archive_link": format!("{LP}/ubuntu/+archive/primary"),
            "distro_series_link": format!("{LP}/ubuntu/noble"),
        }),
    )
}

#[tokio::test]
async fn package_entries_print_the_launchpad_entry() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let series = rebase(series_json("noble", "24.04", "Supported"), &base);
    let ppa = rebase(
        entry(
            "archive",
            "/~jdoe/+archive/ubuntu/ppa",
            json!({"name": "ppa", "description": "Test PPA", "enabled": true, "private": false}),
        ),
        &base,
    );
    let distro = rebase(
        entry(
            "distribution",
            "/ubuntu",
            json!({
                "name": "ubuntu",
                "display_name": "Ubuntu",
                "title": "Ubuntu Linux",
                "summary": "Ubuntu is a Linux distribution.",
                "official_packages": true,
            }),
        ),
        &base,
    );
    let mocks = [
        mock_get(&mut server, "/ubuntu/noble", Matcher::Missing, &series).await,
        mock_get(
            &mut server,
            "/~jdoe/+archive/ubuntu/ppa",
            Matcher::Missing,
            &ppa,
        )
        .await,
        mock_get(&mut server, "/ubuntu", Matcher::Missing, &distro).await,
    ];
    for (name, args, body) in [
        // "24.04" is canonicalised to the "noble" series.
        (
            "package_series",
            &["package", "series", "-s", "24.04"][..],
            &series,
        ),
        (
            "package_ppa",
            &["package", "ppa", "-o", "jdoe", "-p", "ppa"],
            &ppa,
        ),
        ("package_distro", &["package", "distro"], &distro),
    ] {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_modes(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    for mock in mocks {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn package_lists_print_launchpad_collections() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let series = rebase(
        collection(vec![
            series_json("noble", "24.04", "Supported"),
            series_json("questing", "25.10", "Current Stable Release"),
        ]),
        &base,
    );
    // `--limit` fetches one page and keeps Launchpad's continuation link.
    let search = rebase(
        json!({
            "total_size": 3,
            "start": 0,
            "entries": [source_json("hello", "2.10-3", 1)],
            "next_collection_link": format!("{LP}/ubuntu/+archive/primary?ws.op=getPublishedSources&ws.size=1&ws.start=1"),
        }),
        &base,
    );
    let ppa_sources = rebase(
        collection(vec![source_json("hello", "2.10-3ppa1", 2)]),
        &base,
    );
    let mocks = [
        mock_get(&mut server, "/ubuntu/series", Matcher::Missing, &series).await,
        mock_get(
            &mut server,
            "/ubuntu/+archive/primary",
            Matcher::AllOf(vec![
                op("getPublishedSources"),
                Matcher::UrlEncoded("source_name".into(), "hello".into()),
                Matcher::UrlEncoded("pocket".into(), "Updates".into()),
                Matcher::UrlEncoded("distro_series".into(), format!("{base}/ubuntu/noble")),
                Matcher::UrlEncoded("ws.size".into(), "1".into()),
            ]),
            &search,
        )
        .await,
        mock_get(
            &mut server,
            "/~jdoe/+archive/ubuntu/ppa",
            op("getPublishedSources"),
            &ppa_sources,
        )
        .await,
    ];
    for (name, args, body) in [
        (
            "package_list_series",
            &["package", "list-series"][..],
            &series,
        ),
        (
            "package_search",
            &[
                "package", "search", "-s", "noble", "-n", "hello", "-p", "updates", "-l", "1",
            ],
            &search,
        ),
        (
            "package_ppa_sources",
            &["package", "ppa-sources", "-o", "jdoe", "-p", "ppa"],
            &ppa_sources,
        ),
    ] {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_list(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    let report = run(&server, &["package", "list-series"]).await;
    insta::assert_snapshot!("package_list_series_tsv", render(&report, OutputMode::Tsv));
    for mock in mocks {
        mock.expect_at_least(1).assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// package download
// ---------------------------------------------------------------------------

struct DownloadFixture {
    server: mockito::Server,
    dir: tempfile::TempDir,
    mocks: Vec<mockito::Mock>,
}

impl DownloadFixture {
    /// Mock a publication with the given files; `None` bodies fail with 404.
    async fn new(files: &[(&str, Option<&str>)]) -> Self {
        let mut server = isolated_mock_server().await;
        let base = server.url();
        let publication = rebase(source_json("hello", "2.10-3", 1), &base);
        let search = mock_get(
            &mut server,
            "/ubuntu/+archive/primary",
            Matcher::AllOf(vec![
                op("getPublishedSources"),
                Matcher::UrlEncoded("exact_match".into(), "true".into()),
                Matcher::UrlEncoded("source_name".into(), "hello".into()),
            ]),
            &collection(vec![publication]),
        )
        .await;
        let urls: Vec<String> = files
            .iter()
            .map(|(name, _)| format!("{base}/+files/{name}"))
            .collect();
        let list = mock_get(
            &mut server,
            "/ubuntu/+archive/primary/+sourcepub/1",
            op("sourceFileUrls"),
            &json!(urls),
        )
        .await;
        let mut mocks = vec![search, list];
        for (name, body) in files {
            let mock = server.mock("GET", format!("/+files/{name}").as_str());
            let mock = match body {
                Some(body) => mock.with_body(*body),
                None => mock.with_status(404),
            };
            mocks.push(mock.create_async().await);
        }
        Self {
            server,
            dir: tempfile::tempdir().unwrap(),
            mocks,
        }
    }

    fn request(&self) -> SourceDownloadRequest {
        SourceDownloadRequest {
            name: "hello".into(),
            version: None,
            series: "noble".into(),
            distro: "ubuntu".into(),
            output_dir: self.dir.path().join("out"),
        }
    }

    async fn download(&self) -> (SourceDownload, bool) {
        let client = mock_read_client(&self.server, false);
        let mut announced = false;
        let found = package_download(&client, &self.request(), |report| {
            announced = true;
            assert_plain_invariants(&render(report, OutputMode::Plain));
            Ok(())
        })
        .await
        .unwrap();
        (found, announced)
    }

    /// The report JSON with the temporary directory and server redacted.
    fn stable_json(&self, report: &Report) -> Value {
        let dir = self.dir.path().to_string_lossy().into_owned();
        let json = stable(&report.json, &self.server.url());
        serde_json::from_str(&json.to_string().replace(&dir, "[output dir]")).unwrap()
    }
}

#[tokio::test]
async fn package_download_reports_every_file_and_succeeds() {
    let fixture = DownloadFixture::new(&[
        ("hello_2.10-3.dsc", Some("dsc")),
        ("hello_2.10.orig.tar.gz", Some("orig tarball")),
    ])
    .await;
    let (found, announced) = fixture.download().await;
    assert!(announced);
    assert!(package_download_result(&found).is_ok());
    let report = package_download_report(&found);
    assert_eq!(
        report.json["source_package_publishing_history"],
        rebase(source_json("hello", "2.10-3", 1), &fixture.server.url()),
        "the publication must be Launchpad's entry, unchanged"
    );
    assert_eq!(report.json["lpcli"]["downloaded"], 2);
    assert_eq!(report.json["lpcli"]["failed"], 0);
    assert_eq!(
        std::fs::read_to_string(fixture.request().output_dir.join("hello_2.10-3.dsc")).unwrap(),
        "dsc"
    );
    assert_modes(&report);
    insta::assert_snapshot!(
        "package_download_success_plain",
        render(&report, OutputMode::Plain)
            .replace(&*fixture.dir.path().to_string_lossy(), "[output dir]")
    );
    assert_json_report!(
        "package_download_success_json",
        fixture.stable_json(&report)
    );
    for mock in &fixture.mocks {
        mock.assert_async().await;
    }
}

/// Some files downloaded and some failed: the report lists every outcome,
/// and the command then fails with `LpError::Partial` (exit 7).
#[tokio::test]
async fn package_download_partial_failure_is_partial_after_reporting() {
    let fixture = DownloadFixture::new(&[
        ("hello_2.10-3.dsc", Some("dsc")),
        ("hello_2.10.orig.tar.gz", None),
    ])
    .await;
    let (found, _) = fixture.download().await;
    let report = package_download_report(&found);
    assert_eq!(report.json["lpcli"]["downloaded"], 1);
    assert_eq!(report.json["lpcli"]["failed"], 1);
    assert_eq!(report.json["lpcli"]["downloads"][1]["status"], "failed");
    assert!(
        !fixture
            .request()
            .output_dir
            .join("hello_2.10.orig.tar.gz")
            .exists()
    );
    let error = package_download_result(&found).unwrap_err();
    assert!(matches!(error, LpError::Partial(_)), "{error:?}");
    assert_eq!(error.exit_code(), 7);
    let message = error.to_string();
    assert!(message.contains("Downloaded 1 of 2"), "{message}");
    assert!(message.contains("hello_2.10.orig.tar.gz"), "{message}");
    let json: Value = serde_json::from_str(&render(&report, OutputMode::Json)).unwrap();
    assert_eq!(json, report.json);
    let plain = render(&report, OutputMode::Plain);
    assert_plain_invariants(&plain);
    assert!(plain.contains("OK: hello_2.10-3.dsc (3 B)\n"), "{plain}");
    assert!(plain.contains("Error: hello_2.10.orig.tar.gz: "), "{plain}");
    assert!(plain.contains("Warning: 1 file(s) downloaded"), "{plain}");
    assert_json_report!(
        "package_download_partial_json",
        fixture.stable_json(&report)
    );
    for mock in &fixture.mocks {
        mock.assert_async().await;
    }
}

/// Every file failed. Finding the publication and its file list is only
/// preparation and nothing was written, so this is a general error (exit
/// 1), not a partial success. The report is still printed first.
#[tokio::test]
async fn package_download_with_every_file_failed_is_a_general_error() {
    let fixture = DownloadFixture::new(&[("hello_2.10-3.dsc", None)]).await;
    let (found, _) = fixture.download().await;
    let report = package_download_report(&found);
    assert_eq!(report.json["lpcli"]["downloaded"], 0);
    assert_eq!(report.json["lpcli"]["failed"], 1);
    let error = package_download_result(&found).unwrap_err();
    assert!(matches!(error, LpError::Other(_)), "{error:?}");
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("all 1 source file(s)"));
    assert!(
        render(&report, OutputMode::Plain).contains("Error: No files downloaded; 1 failed."),
        "{}",
        render(&report, OutputMode::Plain)
    );
    assert_json_report!(
        "package_download_all_failed_json",
        fixture.stable_json(&report)
    );
}

#[tokio::test]
async fn package_download_without_files_succeeds_and_downloads_nothing() {
    let fixture = DownloadFixture::new(&[]).await;
    let (found, announced) = fixture.download().await;
    assert!(!announced);
    assert!(package_download_result(&found).is_ok());
    assert!(!fixture.request().output_dir.exists());
    let report = package_download_report(&found);
    assert_eq!(report.json["lpcli"]["source_file_urls"], json!([]));
    assert!(render(&report, OutputMode::Plain).starts_with("Warning: No source files found"));
    assert_json_report!(
        "package_download_no_files_json",
        fixture.stable_json(&report)
    );
}

#[tokio::test]
async fn package_download_without_a_publication_is_not_found() {
    let mut server = isolated_mock_server().await;
    let search = mock_get(
        &mut server,
        "/ubuntu/+archive/primary",
        op("getPublishedSources"),
        &collection(vec![]),
    )
    .await;
    let client = mock_read_client(&server, false);
    let request = SourceDownloadRequest {
        name: "missing".into(),
        version: Some("1.0".into()),
        series: "noble".into(),
        distro: "ubuntu".into(),
        output_dir: "unused".into(),
    };
    let error = package_download(&client, &request, |_| panic!("nothing to announce"))
        .await
        .unwrap_err();
    assert_eq!(error.exit_code(), 4);
    assert!(error.to_string().contains("'missing' (1.0)"), "{error}");
    search.assert_async().await;
}

#[test]
fn package_download_is_not_a_read_only_report() {
    let client = LaunchpadClient::new(None).with_base_url("http://127.0.0.1:9");
    let Command::Package(cmd) = parsed(&["package", "download", "-n", "hello", "-s", "noble"])
    else {
        unreachable!()
    };
    let error = tokio_test::block_on(package_report(&client, cmd)).unwrap_err();
    assert_eq!(error.exit_code(), 2);
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

fn milestone_json(name: &str, active: bool) -> Value {
    entry(
        "milestone",
        &format!("/launchpad/+milestone/{name}"),
        json!({
            "name": name,
            "title": format!("Launchpad {name}"),
            "code_name": null,
            "is_active": active,
            "date_targeted": "2025-06-01",
            "target_link": format!("{LP}/launchpad"),
        }),
    )
}

fn project_series_json(name: &str) -> Value {
    entry(
        "project_series",
        &format!("/launchpad/{name}"),
        json!({
            "name": name,
            "title": format!("Launchpad {name} series"),
            "summary": "The main line.",
            "status": "Active Development",
            "date_created": "2004-09-24T20:58:00+00:00",
        }),
    )
}

#[tokio::test]
async fn project_commands_print_launchpad_json() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let project = rebase(
        entry(
            "project",
            "/launchpad",
            json!({
                "name": "launchpad",
                "display_name": "Launchpad",
                "summary": "Launchpad is a software collaboration platform.",
                "homepage_url": "https://launchpad.net/",
                "active": true,
                "date_created": "2004-09-24T20:58:00+00:00",
                "brand_new_field": [1, 2, 3],
            }),
        ),
        &base,
    );
    let search = rebase(collection(vec![project.clone()]), &base);
    let milestones = rebase(
        collection(vec![
            milestone_json("1.0", false),
            milestone_json("2.0", true),
        ]),
        &base,
    );
    let active = rebase(collection(vec![milestone_json("2.0", true)]), &base);
    let milestone = rebase(milestone_json("1.0", false), &base);
    let series = rebase(project_series_json("trunk"), &base);
    let all_series = rebase(collection(vec![project_series_json("trunk")]), &base);
    let releases = rebase(
        collection(vec![entry(
            "project_release",
            "/launchpad/trunk/1.0",
            json!({
                "version": "1.0",
                "date_released": "2025-06-02T10:00:00+00:00",
                "release_notes": "First release.",
                "changelog": null,
                "milestone_link": format!("{LP}/launchpad/+milestone/1.0"),
            }),
        )]),
        &base,
    );
    let mocks = [
        mock_get(&mut server, "/launchpad", Matcher::Missing, &project).await,
        mock_get(&mut server, "/projects", op("search"), &search).await,
        mock_get(
            &mut server,
            "/launchpad/all_milestones",
            Matcher::Missing,
            &milestones,
        )
        .await,
        mock_get(
            &mut server,
            "/launchpad/active_milestones",
            Matcher::Missing,
            &active,
        )
        .await,
        mock_get(
            &mut server,
            "/launchpad/+milestone/1.0",
            Matcher::Missing,
            &milestone,
        )
        .await,
        mock_get(&mut server, "/launchpad/trunk", Matcher::Missing, &series).await,
        mock_get(
            &mut server,
            "/launchpad/series",
            Matcher::Missing,
            &all_series,
        )
        .await,
        mock_get(
            &mut server,
            "/launchpad/trunk/releases",
            Matcher::Missing,
            &releases,
        )
        .await,
    ];
    let entries: [(&str, &[&str], &Value); 3] = [
        (
            "project_show",
            &["project", "show", "-n", "launchpad"],
            &project,
        ),
        (
            "project_show_milestone",
            &["project", "show-milestone", "-p", "launchpad", "-n", "1.0"],
            &milestone,
        ),
        (
            "project_series_show",
            &["project", "series-show", "-p", "launchpad", "-s", "trunk"],
            &series,
        ),
    ];
    for (name, args, body) in entries {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_modes(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    let lists: [(&str, &[&str], &Value); 5] = [
        (
            "project_search",
            &["project", "search", "-q", "launchpad"],
            &search,
        ),
        (
            "project_milestones",
            &["project", "milestones", "-p", "launchpad"],
            &milestones,
        ),
        (
            "project_milestones_active",
            &["project", "milestones", "-p", "launchpad", "--active"],
            &active,
        ),
        (
            "project_list_series",
            &["project", "list-series", "-p", "launchpad"],
            &all_series,
        ),
        (
            "project_series_releases",
            &[
                "project",
                "series-releases",
                "-p",
                "launchpad",
                "-s",
                "trunk",
            ],
            &releases,
        ),
    ];
    for (name, args, body) in lists {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_list(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    let report = run(&server, &["project", "milestones", "-p", "launchpad"]).await;
    insta::assert_snapshot!("project_milestones_tsv", render(&report, OutputMode::Tsv));
    for mock in mocks {
        mock.expect_at_least(1).assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// cve
// ---------------------------------------------------------------------------

fn cve_json(sequence: &str, title: &str) -> Value {
    entry(
        "cve",
        &format!("/bugs/cve/{sequence}"),
        json!({
            "sequence": sequence,
            "title": title,
            "status": "Entry",
            "description": format!("{title} description."),
            "url": format!("https://www.cve.org/CVERecord?id=CVE-{sequence}"),
            "date_made_public": "2024-01-02T00:00:00+00:00",
            "date_created": "2024-01-03T00:00:00+00:00",
        }),
    )
}

#[tokio::test]
async fn cve_commands_print_launchpad_json() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let cve = rebase(cve_json("2024-1234", "Curl overflow"), &base);
    let linked = rebase(
        collection(vec![cve_json("2024-1234", "Curl overflow")]),
        &base,
    );
    let search_page = rebase(
        json!({
            "total_size": 2,
            "start": 0,
            "entries": [cve_json("2024-1234", "Curl overflow"), cve_json("2024-9999", "Other bug")],
        }),
        &base,
    );
    let mocks = [
        mock_get(&mut server, "/bugs/cve/2024-1234", Matcher::Missing, &cve).await,
        mock_get(&mut server, "/bugs/1/cves", Matcher::Missing, &linked).await,
    ];
    let search = server
        .mock("GET", "/bugs/cve")
        .match_query(Matcher::AllOf(vec![
            op("advancedSearch"),
            Matcher::UrlEncoded("in_distribution".into(), format!("{base}/ubuntu")),
            Matcher::UrlEncoded("limit".into(), "25".into()),
        ]))
        .with_body(search_page.to_string())
        .expect(2)
        .create_async()
        .await;

    let report = run(&server, &["cve", "show", "-s", "2024-1234"]).await;
    assert_eq!(report.json, cve);
    assert_modes(&report);
    assert_json_report!("cve_show_json", stable(&report.json, &base));

    let report = run(&server, &["cve", "bug-cves", "-b", "1"]).await;
    assert_eq!(report.json, linked);
    assert_list(&report);
    assert_json_report!("cve_bug_cves_json", stable(&report.json, &base));

    // Unfiltered: every CVE Launchpad sent, with an exact count because
    // fewer than `--limit` came back.
    let report = run(&server, &["cve", "search"]).await;
    assert_eq!(report.json["entries"], search_page["entries"]);
    assert_eq!(report.json["total_size"], 2);
    assert!(report.json.get("next_collection_link").is_none());
    assert_list(&report);
    assert_json_report!("cve_search_json", stable(&report.json, &base));

    // The keyword filter runs locally, so the count is the number of matches.
    let report = run(&server, &["cve", "search", "-k", "CURL"]).await;
    assert_eq!(report.json["entries"], json!([cve]));
    assert_eq!(report.json["total_size"], 1);
    assert_list(&report);
    insta::assert_snapshot!("cve_search_keyword_tsv", render(&report, OutputMode::Tsv));
    for mock in mocks {
        mock.assert_async().await;
    }
    search.assert_async().await;
}

#[test]
fn incomplete_cve_searches_omit_total_size() {
    let found = cves::CveSearch {
        cves: vec![Raw::from_value(cve_json("2024-1234", "Curl overflow")).unwrap()],
        complete: false,
    };
    let report = cve_search_report(&found);
    assert!(report.json.get("total_size").is_none());
    assert!(report.json.get("next_collection_link").is_none());
    assert_eq!(report.json["start"], 0);
    assert_eq!(
        report.json["entries"][0],
        cve_json("2024-1234", "Curl overflow")
    );
}

// ---------------------------------------------------------------------------
// spec
// ---------------------------------------------------------------------------

fn spec_json(name: &str) -> Value {
    entry(
        "specification",
        &format!("/launchpad/+spec/{name}"),
        json!({
            "name": name,
            "title": format!("Spec {name}"),
            "summary": "Make things better.",
            "priority": "High",
            "lifecycle_status": "Started",
            "implementation_status": "Good progress",
            "definition_status": "Approved",
            "specification_url": null,
            "date_created": "2020-05-06T07:08:09+00:00",
        }),
    )
}

#[tokio::test]
async fn spec_commands_print_launchpad_json() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let spec = rebase(spec_json("a-spec"), &base);
    let valid = rebase(collection(vec![spec_json("a-spec")]), &base);
    let all = rebase(
        collection(vec![spec_json("a-spec"), spec_json("old-spec")]),
        &base,
    );
    let mocks = [
        mock_get(
            &mut server,
            "/launchpad/+spec/a-spec",
            Matcher::Missing,
            &spec,
        )
        .await,
        mock_get(
            &mut server,
            "/launchpad/valid_specifications",
            Matcher::Missing,
            &valid,
        )
        .await,
        mock_get(
            &mut server,
            "/launchpad/all_specifications",
            Matcher::Missing,
            &all,
        )
        .await,
    ];
    let report = run(
        &server,
        &["spec", "show", "-t", "launchpad", "-n", "a-spec"],
    )
    .await;
    assert_eq!(report.json, spec);
    assert_modes(&report);
    insta::assert_snapshot!("spec_show_plain", render(&report, OutputMode::Plain));
    assert_json_report!("spec_show_json", stable(&report.json, &base));
    let lists: [(&str, &[&str], &Value); 2] = [
        ("spec_list", &["spec", "list", "-t", "launchpad"], &valid),
        (
            "spec_list_all",
            &["spec", "list", "-t", "launchpad", "--all"],
            &all,
        ),
    ];
    for (name, args, body) in lists {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_list(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    for mock in mocks {
        mock.assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// question
// ---------------------------------------------------------------------------

fn question_json(id: u64, title: &str) -> Value {
    entry(
        "question",
        &format!("/ubuntu/+question/{id}"),
        json!({
            "id": id,
            "title": title,
            "description": "How do I do this?",
            "status": "Open",
            "owner_link": format!("{LP}/~jdoe"),
            "target_link": format!("{LP}/ubuntu"),
            "answerer_link": null,
            "date_created": "2025-01-01T12:00:00+00:00",
            "date_last_updated": "2025-01-02T12:00:00+00:00",
        }),
    )
}

#[tokio::test]
async fn question_commands_print_launchpad_json() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let question = rebase(question_json(7, "Install help"), &base);
    let search = rebase(
        collection(vec![
            question_json(7, "Install help"),
            question_json(8, "Boot help"),
        ]),
        &base,
    );
    let messages = rebase(
        collection(vec![
            entry(
                "question_message",
                "/ubuntu/+question/7/messages/1",
                json!({
                    "index": 1,
                    "content": "Try this.\nThen that.",
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
    let mocks = [
        mock_get(
            &mut server,
            "/questions",
            Matcher::AllOf(vec![
                op("getByID"),
                Matcher::UrlEncoded("question_id".into(), "7".into()),
            ]),
            &question,
        )
        .await,
        mock_get(
            &mut server,
            "/ubuntu",
            Matcher::AllOf(vec![
                op("searchQuestions"),
                Matcher::UrlEncoded("search_text".into(), "help".into()),
                Matcher::UrlEncoded("status".into(), "Open".into()),
            ]),
            &search,
        )
        .await,
        mock_get(
            &mut server,
            "/ubuntu/+question/7/messages",
            Matcher::Missing,
            &messages,
        )
        .await,
    ];
    let report = run(&server, &["question", "show", "-q", "7"]).await;
    assert_eq!(report.json, question);
    assert_modes(&report);
    assert_json_report!("question_show_json", stable(&report.json, &base));

    let report = run(
        &server,
        &[
            "question", "search", "-t", "ubuntu", "-q", "help", "-x", "Open",
        ],
    )
    .await;
    assert_eq!(report.json, search);
    assert_list(&report);
    assert_eq!(
        render(&report, OutputMode::Tsv).lines().nth(2),
        Some("8\tBoot help\tOpen")
    );
    assert_json_report!("question_search_json", stable(&report.json, &base));

    let report = run(
        &server,
        &["question", "messages", "-t", "ubuntu", "-q", "7"],
    )
    .await;
    assert_eq!(report.json, messages);
    assert_list(&report);
    insta::assert_snapshot!(
        "question_messages_plain",
        render(&report, OutputMode::Plain)
    );
    insta::assert_snapshot!("question_messages_tsv", render(&report, OutputMode::Tsv));
    assert_json_report!("question_messages_json", stable(&report.json, &base));
    for mock in mocks {
        mock.assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// translation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn translation_commands_print_launchpad_json() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let queue = rebase(
        collection(vec![entry(
            "translation_import_queue_entry",
            "/+imports/1",
            json!({
                "path": "po/hello.pot",
                "status": "Needs Review",
                "date_created": "2025-03-04T05:06:07+00:00",
                "uploader_link": format!("{LP}/~jdoe"),
                "sourcepackage_link": format!("{LP}/ubuntu/noble/+source/hello"),
            }),
        )]),
        &base,
    );
    let templates = rebase(
        collection(vec![entry(
            "translation_template",
            "/ubuntu/noble/+source/hello/+pots/hello",
            json!({
                "name": "hello",
                "path": "po/hello.pot",
                "priority": 100,
                "is_current": true,
                "is_obsolete": false,
                "date_last_updated": "2025-03-04T05:06:07+00:00",
            }),
        )]),
        &base,
    );
    let mocks = [
        mock_get(
            &mut server,
            "/ubuntu/noble",
            op("getTranslationImportQueueEntries"),
            &queue,
        )
        .await,
        mock_get(
            &mut server,
            "/ubuntu/noble",
            op("getTranslationTemplates"),
            &templates,
        )
        .await,
    ];
    let lists: [(&str, &[&str], &Value); 2] = [
        (
            "translation_queue",
            &["translation", "queue", "-s", "noble"],
            &queue,
        ),
        // "24.04" is canonicalised to the "noble" series.
        (
            "translation_templates",
            &["translation", "templates", "-s", "24.04"],
            &templates,
        ),
    ];
    for (name, args, body) in lists {
        let report = run(&server, args).await;
        assert_eq!(&report.json, body, "{name}");
        assert_list(&report);
        assert_json_report!(format!("{name}_json"), stable(&report.json, &base));
    }
    for mock in mocks {
        mock.assert_async().await;
    }
}

// ---------------------------------------------------------------------------
// Empty lists and pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_lists_print_an_empty_collection_and_message() {
    let mut server = isolated_mock_server().await;
    let empty = json!({"total_size": 0, "start": 0, "entries": []});
    let mock = mock_get(
        &mut server,
        "/launchpad/valid_specifications",
        Matcher::Missing,
        &empty,
    )
    .await;
    let report = run(&server, &["spec", "list", "-t", "launchpad"]).await;
    assert_eq!(report.json, empty);
    assert_eq!(render(&report, OutputMode::Plain), "No specs found.\n");
    assert_eq!(
        render(&report, OutputMode::Tsv),
        "name\ttitle\tpriority\tlifecycle_status\timplementation_status\n"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn fetched_collections_merge_pages_and_omit_the_continuation_link() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let first = server
        .mock("GET", "/launchpad/series")
        .match_query(Matcher::Missing)
        .with_body(
            json!({
                "total_size": 2,
                "start": 0,
                "entries": [rebase(project_series_json("trunk"), &base)],
                "next_collection_link": format!("{base}/launchpad/series?ws.start=1&ws.size=1"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let second = server
        .mock("GET", "/launchpad/series")
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_body(
            json!({"start": 1, "entries": [rebase(project_series_json("2.0"), &base)]}).to_string(),
        )
        .create_async()
        .await;
    let report = run(&server, &["project", "list-series", "-p", "launchpad"]).await;
    assert_eq!(
        report.json,
        json!({
            "total_size": 2,
            "start": 0,
            "entries": [
                rebase(project_series_json("trunk"), &base),
                rebase(project_series_json("2.0"), &base),
            ],
        })
    );
    assert_list(&report);
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn launchpad_errors_propagate_with_their_exit_codes() {
    let mut server = isolated_mock_server().await;
    let missing = server
        .mock("GET", "/~nobody")
        .with_status(404)
        .with_body("Object: <Person>, name: 'nobody'")
        .create_async()
        .await;
    let client = mock_read_client(&server, false);
    let Command::Person(cmd) = parsed(&["person", "show", "-n", "nobody"]) else {
        unreachable!()
    };
    let error = person_report(&client, cmd).await.unwrap_err();
    assert_eq!(error.exit_code(), 4);
    let rendered = Output::fixed(OutputMode::Json, false, 100).render_error(&error);
    let value: Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["error"]["kind"], "not_found");
    assert_eq!(value["error"]["http_status"], 404);
    missing.assert_async().await;
}
