//! Phase 1 CLI tests for `lpcli package build show | logs | retry`.

#[allow(unused_imports)]
use super::*;
use crate::output::tests::assert_plain_invariants;
use lpcli::error::LpError;
use mockito::Matcher;
use serde_json::{Value, json};

const LP: &str = "https://api.launchpad.net/devel";

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

fn stable(value: &Value, base: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(base, LP)),
        Value::Array(values) => values.iter().map(|v| stable(v, base)).collect(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.replace(base, LP), stable(value, base)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
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

macro_rules! assert_json_snapshot_redacted {
    ($name:expr, $value:expr) => {
        insta::assert_json_snapshot!($name, $value, {
            ".**.datecreated" => "[timestamp]",
            ".**.date_first_dispatched" => "[timestamp]",
            ".**.date_started" => "[timestamp]",
            ".**.datebuilt" => "[timestamp]",
            ".lpcli.output_directory" => "[output directory]",
            ".lpcli.downloads[].path" => "[output path]",
            ".lpcli.saved_logs[].path" => "[output path]",
        })
    };
}

fn ppa_build_json(
    id: u64,
    series: &str,
    arch: &str,
    state: &str,
    can_be_retried: bool,
    log_path: Option<&str>,
) -> Value {
    json!({
        "self_link": format!("{LP}/~jdoe/+archive/ubuntu/test/+build/{id}"),
        "web_link": format!("https://launchpad.net/~jdoe/+archive/ubuntu/test/+build/{id}"),
        "resource_type_link": format!("{LP}/#build"),
        "http_etag": "\"etag-build\"",
        "title": format!("{arch} build of mutter 50.1-1~ppa1 in ubuntu {series} RELEASE"),
        "arch_tag": arch,
        "archive_link": format!("{LP}/~jdoe/+archive/ubuntu/test"),
        "builder_link": format!("{LP}/builders/bos03-{arch}-01"),
        "buildstate": state,
        "can_be_retried": can_be_retried,
        "can_be_cancelled": false,
        "can_be_rescored": false,
        "datecreated": "2026-10-08T08:59:00+00:00",
        "date_first_dispatched": "2026-10-08T09:00:00+00:00",
        "date_started": "2026-10-08T09:00:00+00:00",
        "datebuilt": "2026-10-08T09:04:54.281155+00:00",
        "duration": "0:04:54.281155",
        "build_log_url": log_path.map(|p| format!("{LP}{p}")),
        "upload_log_url": null,
        "buildinfo_url": null,
        "changesfile_url": null,
        "source_package_name": "mutter",
        "source_package_version": "50.1-1~ppa1",
        "distro_series_link": format!("{LP}/ubuntu/{series}"),
        "current_source_publication_link": format!("{LP}/~jdoe/+archive/ubuntu/test/+sourcepub/1"),
        "pocket": "Release",
        "dependencies": null,
        "external_dependencies": null,
        "score": null,
    })
}

#[tokio::test]
async fn package_build_show_renders_ppa_build() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let body = rebase(
        ppa_build_json(
            33961943,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/buildlog_ubuntu-noble-amd64.mutter_50.1-1~ppa1_BUILDING.txt.gz"),
        ),
        &base,
    );
    let mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/33961943")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let report = build_show_report(
        &client,
        Some(33961943),
        None,
        None,
        Some("jdoe/test"),
        "ubuntu",
    )
    .await
    .unwrap();

    assert_eq!(report.json, body);
    assert_modes(&report);
    assert_json_snapshot_redacted!("package_build_show_json", stable(&report.json, &base));
    insta::assert_snapshot!(
        "package_build_show_plain",
        stable_text(&render(&report, OutputMode::Plain), &base)
    );
    insta::assert_snapshot!(
        "package_build_show_rich",
        stable_text(&render(&report, OutputMode::Rich), &base)
    );
    mock.assert_async().await;
}

fn stable_text(text: &str, base: &str) -> String {
    text.replace(base, LP)
}

#[tokio::test]
async fn package_build_logs_downloads_public_logs_skips_missing_and_reports_partial() {
    let mut server = isolated_mock_server().await;
    let base = server.url();

    let ppa_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "name": "test",
                "self_link": format!("{base}/~jdoe/+archive/ubuntu/test"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let b1 = rebase(
        ppa_build_json(
            101,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/shared_log.txt.gz"),
        ),
        &base,
    );
    let mut b2 = rebase(
        ppa_build_json(
            102,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/shared_log.txt.gz"),
        ),
        &base,
    );
    b2["upload_log_url"] = json!(format!("{base}/+files/missing_upload.log"));
    let b3 = rebase(
        ppa_build_json(103, "noble", "amd64", "Failed to build", true, None),
        &base,
    );

    let records_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("build_state".into(), "Failed to build".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"start": 0, "entries": [b1, b2, b3]}).to_string())
        .create_async()
        .await;

    let log_ok = server
        .mock("GET", "/+files/shared_log.txt.gz")
        .match_header("authorization", Matcher::Missing)
        .with_status(200)
        .with_body(b"build-log-content")
        .expect(2)
        .create_async()
        .await;
    let upload_missing = server
        .mock("GET", "/+files/missing_upload.log")
        .match_header("authorization", Matcher::Missing)
        .with_status(404)
        .create_async()
        .await;

    let dir = tempfile::tempdir().unwrap();
    let client = mock_read_client(&server, true);
    let request = BuildLogsRequest {
        target: BuildTarget::Filter(BuildFilterRequest {
            owner: None,
            ppa: Some("jdoe/test".into()),
            name: None,
            version: None,
            series: Some("noble".into()),
            arch: vec!["amd64".into()],
            state: vec!["Failed to build".into()],
            exclude_state: Vec::new(),
            pocket: None,
            distro: "ubuntu".into(),
            limit: 30,
        }),
        output_dir: dir.path().to_path_buf(),
        include_upload_logs: true,
    };

    let executed = execute_build_logs(
        &client,
        &request,
        lpcli::download::ProgressMode::None,
        |_| Ok(()),
    )
    .await
    .unwrap();
    let report = build_logs_report(&executed);
    assert_eq!(report.json["lpcli"]["downloaded"], 2);
    assert_eq!(report.json["lpcli"]["failed"], 1);
    assert_eq!(report.json["lpcli"]["skipped_no_log"], 1);
    assert!(dir.path().join("build-101_shared_log.txt.gz").exists());
    assert!(dir.path().join("build-102_shared_log.txt.gz").exists());

    assert_modes(&report);
    assert_json_snapshot_redacted!(
        "package_build_logs_partial_json",
        stable(&report.json, &base)
    );

    let err = build_logs_result(&executed).unwrap_err();
    assert!(matches!(err, LpError::Partial(_)), "{err:?}");

    ppa_mock.assert_async().await;
    records_mock.assert_async().await;
    log_ok.assert_async().await;
    upload_missing.assert_async().await;
}

#[tokio::test]
async fn package_build_retry_single_eligible_and_ineligible_and_quiet() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let eligible_body = rebase(
        ppa_build_json(
            201,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/b201.txt.gz"),
        ),
        &base,
    );
    let retried_body = rebase(
        ppa_build_json(201, "noble", "amd64", "Needs building", false, None),
        &base,
    );
    let ineligible_body = rebase(
        ppa_build_json(202, "noble", "amd64", "Successfully built", false, None),
        &base,
    );

    let get_201 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/201")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(eligible_body.to_string())
        .expect(1)
        .create_async()
        .await;
    let post_201 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/201")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "retry".into()))
        .with_status(200)
        .create_async()
        .await;
    let readback_201 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/201")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(retried_body.to_string())
        .expect(1)
        .create_async()
        .await;

    let get_202 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/202")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(ineligible_body.to_string())
        .create_async()
        .await;
    let no_post_202 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/202")
        .expect(0)
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let req_ok = BuildRetryRequest {
        target: BuildTarget::Single {
            build_id: Some(201),
            build_url: None,
            owner: None,
            ppa: Some("jdoe/test".into()),
            distro: "ubuntu".into(),
        },
        dry_run: false,
        yes: false,
        save_logs: None,
    };
    let outcome = execute_build_retry(&client, false, &req_ok, |_| {
        unreachable!("no prompt for single")
    })
    .await
    .unwrap();
    outcome.result.unwrap();
    assert_eq!(
        outcome.retried_links,
        vec![format!("{base}/~jdoe/+archive/ubuntu/test/+build/201")]
    );
    assert_modes(&outcome.report);
    assert_json_snapshot_redacted!(
        "package_build_retry_single_json",
        stable(&outcome.report.json, &base)
    );
    insta::assert_snapshot!(
        "package_build_retry_single_plain",
        stable_text(&render(&outcome.report, OutputMode::Plain), &base)
    );

    let req_bad = BuildRetryRequest {
        target: BuildTarget::Single {
            build_id: Some(202),
            build_url: None,
            owner: None,
            ppa: Some("jdoe/test".into()),
            distro: "ubuntu".into(),
        },
        dry_run: false,
        yes: false,
        save_logs: None,
    };
    let err = execute_build_retry(&client, false, &req_bad, |_| unreachable!())
        .await
        .unwrap_err();
    assert!(matches!(err, LpError::Usage(_)), "{err:?}");

    get_201.assert_async().await;
    post_201.assert_async().await;
    readback_201.assert_async().await;
    get_202.assert_async().await;
    no_post_202.assert_async().await;
}

#[tokio::test]
async fn package_build_retry_bulk_dry_run_confirm_and_save_logs_prevents_retry_on_backup_failure() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let dir = tempfile::tempdir().unwrap();

    let ppa_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "name": "test",
                "self_link": format!("{base}/~jdoe/+archive/ubuntu/test"),
            })
            .to_string(),
        )
        .expect(3)
        .create_async()
        .await;

    // 301 matches noble/amd64 failed + retryable + good log.
    let b301 = rebase(
        ppa_build_json(
            301,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/b301.txt.gz"),
        ),
        &base,
    );
    // 302 matches noble/amd64 failed + retryable, but log backup returns 500 -> MUST NOT be retried!
    let b302 = rebase(
        ppa_build_json(
            302,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/b302_fail.txt.gz"),
        ),
        &base,
    );
    // 303 matches noble/amd64 failed, but `can_be_retried: false` -> skipped.
    let b303 = rebase(
        ppa_build_json(303, "noble", "amd64", "Failed to build", false, None),
        &base,
    );
    // 304 is Dependency wait / jammy / arm64 -> filtered out by series/arch/exclude-state.
    let b304 = rebase(
        ppa_build_json(304, "jammy", "arm64", "Dependency wait", true, None),
        &base,
    );

    let records_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("build_state".into(), "Failed to build".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"start": 0, "entries": [b301, b302, b303, b304]}).to_string())
        .expect(3)
        .create_async()
        .await;

    let log_301 = server
        .mock("GET", "/+files/b301.txt.gz")
        .match_header("authorization", Matcher::Missing)
        .with_status(200)
        .with_body(b"saved-301")
        .create_async()
        .await;
    let log_302_fail = server
        .mock("GET", "/+files/b302_fail.txt.gz")
        .with_status(500)
        .with_body("storage error")
        .create_async()
        .await;

    let post_301 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/301")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "retry".into()))
        .with_status(200)
        .create_async()
        .await;
    let readback_301 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/301")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            rebase(
                ppa_build_json(301, "noble", "amd64", "Needs building", false, None),
                &base,
            )
            .to_string(),
        )
        .create_async()
        .await;

    // Critical regression guard: 302 must receive zero POST retry requests because saving its log failed!
    let no_post_302 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/302")
        .expect(0)
        .create_async()
        .await;
    let no_post_303 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/303")
        .expect(0)
        .create_async()
        .await;
    let no_post_304 = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/304")
        .expect(0)
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let save_dir = dir.path().join("backup-logs");
    let target = BuildTarget::Filter(BuildFilterRequest {
        owner: None,
        ppa: Some("jdoe/test".into()),
        name: Some("mutter".into()),
        version: None,
        series: Some("noble".into()),
        arch: vec!["amd64".into()],
        state: vec!["Failed to build".into()],
        exclude_state: vec!["Dependency wait".into()],
        pocket: None,
        distro: "ubuntu".into(),
        limit: 30,
    });

    // 1. `--dry-run` does not create backup dir, prompt, or POST anything.
    let dry_run = execute_build_retry(
        &client,
        false,
        &BuildRetryRequest {
            target: target.clone(),
            dry_run: true,
            yes: false,
            save_logs: Some(save_dir.clone()),
        },
        |_| unreachable!("dry-run must not prompt"),
    )
    .await
    .unwrap();
    dry_run.result.unwrap();
    assert!(
        !save_dir.exists(),
        "dry-run must not create --save-logs dir"
    );
    assert!(dry_run.retried_links.is_empty());
    assert_eq!(dry_run.report.json["lpcli"]["dry_run"], true);
    assert_eq!(
        dry_run.report.json["lpcli"]["eligible"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        dry_run.report.json["lpcli"]["skipped"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_modes(&dry_run.report);
    assert_json_snapshot_redacted!(
        "package_build_retry_dry_run_json",
        stable(&dry_run.report.json, &base)
    );
    insta::assert_snapshot!(
        "package_build_retry_dry_run_plain",
        stable_text(&render(&dry_run.report, OutputMode::Plain), &base)
    );

    // 2. Interactive confirmation declined cancels cleanly without POST or disk writes.
    let declined = execute_build_retry(
        &client,
        false,
        &BuildRetryRequest {
            target: target.clone(),
            dry_run: false,
            yes: false,
            save_logs: Some(save_dir.clone()),
        },
        |candidates| {
            assert_eq!(candidates.len(), 2);
            Ok(false)
        },
    )
    .await
    .unwrap();
    declined.result.unwrap();
    assert_eq!(declined.report.json["lpcli"]["cancelled"], true);
    assert!(!save_dir.exists());
    assert_modes(&declined.report);
    assert_json_snapshot_redacted!(
        "package_build_retry_cancelled_json",
        stable(&declined.report.json, &base)
    );

    // 3. Pre-create existing `build-301_b301.txt.gz` so we also verify collision-safe naming!
    std::fs::create_dir_all(&save_dir).unwrap();
    std::fs::write(save_dir.join("build-301_b301.txt.gz"), b"previous-backup").unwrap();

    // 4. Execute with interactive accept: 301 saves log (with -2 suffix) then retries;
    //    302 fails log backup and skips POST retry; 303 is skipped -> Partial exit 7.
    let executed = execute_build_retry(
        &client,
        false,
        &BuildRetryRequest {
            target,
            dry_run: false,
            yes: false,
            save_logs: Some(save_dir.clone()),
        },
        |_| Ok(true),
    )
    .await
    .unwrap();
    assert!(
        matches!(executed.result, Err(LpError::Partial(_))),
        "{:?}",
        executed.result
    );
    assert_eq!(
        executed.retried_links,
        vec![format!("{base}/~jdoe/+archive/ubuntu/test/+build/301")]
    );
    assert_eq!(
        std::fs::read(save_dir.join("build-301_b301.txt.gz")).unwrap(),
        b"previous-backup"
    );
    assert_eq!(
        std::fs::read(save_dir.join("build-301_b301-2.txt.gz")).unwrap(),
        b"saved-301"
    );
    assert_eq!(
        executed.report.json["lpcli"]["failures"][0]["operation"],
        "save_log"
    );
    assert_modes(&executed.report);
    assert_json_snapshot_redacted!(
        "package_build_retry_partial_save_logs_json",
        stable(&executed.report.json, &base)
    );

    for mock in [
        ppa_mock,
        records_mock,
        log_301,
        log_302_fail,
        post_301,
        readback_301,
        no_post_302,
        no_post_303,
        no_post_304,
    ] {
        mock.assert_async().await;
    }
}

#[test]
fn bulk_retry_confirmation_accepts_short_yes_and_rejects_oversized_lines() {
    for accept in ["y\n", "YES\n", "  yes  \n", "yes"] {
        let mut cursor = std::io::Cursor::new(accept.as_bytes());
        assert!(
            read_confirmation_from(&mut cursor).unwrap(),
            "expected accept for {accept:?}"
        );
    }

    let padded_yesno = format!("{}yesno\n", " ".repeat(61));
    let padded_yes = format!("{}yes\n", " ".repeat(64));
    for reject in [
        "",
        "no\n",
        "yesno\n",
        padded_yesno.as_str(),
        padded_yes.as_str(),
    ] {
        let mut cursor = std::io::Cursor::new(reject.as_bytes());
        assert!(
            !read_confirmation_from(&mut cursor).unwrap(),
            "expected reject for len={}: {reject:?}",
            reject.len()
        );
    }
}

#[tokio::test]
async fn ppa_build_records_pagination_detects_cycles_and_enforces_page_cap() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let ppa_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::Missing)
        .with_status(200)
        .with_body(
            json!({
                "name": "test",
                "self_link": format!("{base}/~jdoe/+archive/ubuntu/test"),
            })
            .to_string(),
        )
        .expect(2)
        .create_async()
        .await;

    let arm_entry = rebase(
        ppa_build_json(401, "noble", "arm64", "Failed to build", true, None),
        &base,
    );
    let page1_url = "/~jdoe/+archive/ubuntu/test?ws.op=getBuildRecords&ws.start=0";
    let page2_url = "/~jdoe/+archive/ubuntu/test?ws.op=getBuildRecords&ws.start=50";

    // Cycle test: page 1 -> page 2 -> page 1 -> cyclic error on repeat visit.
    let p1 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.start".into(), "0".into()),
        ]))
        .with_status(200)
        .with_body(
            json!({
                "start": 0,
                "entries": [arm_entry],
                "next_collection_link": format!("{base}{page2_url}"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let p2 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.start".into(), "50".into()),
        ]))
        .with_status(200)
        .with_body(
            json!({
                "start": 50,
                "entries": [arm_entry],
                "next_collection_link": format!("{base}{page1_url}"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let initial_cyclic = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.size".into(), "50".into()),
        ]))
        .with_status(200)
        .with_body(
            json!({
                "start": 0,
                "entries": [arm_entry],
                "next_collection_link": format!("{base}{page2_url}"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let ppa = PpaRef {
        owner: "jdoe".into(),
        name: "test".into(),
    };
    let filter = BuildFilterRequest {
        owner: None,
        ppa: Some("jdoe/test".into()),
        name: None,
        version: None,
        series: Some("noble".into()),
        arch: vec!["amd64".into()],
        state: vec!["Failed to build".into()],
        exclude_state: Vec::new(),
        pocket: None,
        distro: "ubuntu".into(),
        limit: 10,
    };

    let cycle_err = resolve_ppa_build_records_filtered_with_max_pages(
        &client,
        &ppa,
        &filter,
        Some("noble"),
        10,
    )
    .await
    .err()
    .expect("cyclic pagination should error");
    assert!(cycle_err.to_string().contains("cyclic"), "{cycle_err}");

    p1.assert_async().await;
    p2.assert_async().await;
    initial_cyclic.assert_async().await;

    // Max-pages cap test: stops after max_pages=2 without fetching page 3.
    let page3_url = "/~jdoe/+archive/ubuntu/test?ws.op=getBuildRecords&ws.start=100";
    let cap_init = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.size".into(), "50".into()),
        ]))
        .with_status(200)
        .with_body(
            json!({
                "start": 0,
                "entries": [arm_entry],
                "next_collection_link": format!("{base}{page2_url}"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let cap_p2 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.start".into(), "50".into()),
        ]))
        .with_status(200)
        .with_body(
            json!({
                "start": 50,
                "entries": [arm_entry],
                "next_collection_link": format!("{base}{page3_url}"),
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let no_p3 = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getBuildRecords".into()),
            Matcher::UrlEncoded("ws.start".into(), "100".into()),
        ]))
        .expect(0)
        .create_async()
        .await;

    let capped =
        resolve_ppa_build_records_filtered_with_max_pages(&client, &ppa, &filter, Some("noble"), 2)
            .await
            .unwrap();
    assert!(!capped.complete);
    assert!(capped.builds.is_empty());

    ppa_mock.assert_async().await;
    cap_init.assert_async().await;
    cap_p2.assert_async().await;
    no_p3.assert_async().await;
}

#[tokio::test]
async fn build_logs_preserves_existing_files_names_upload_logs_and_redacts_bad_urls() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let dir = tempfile::tempdir().unwrap();

    let mut b1 = rebase(
        ppa_build_json(
            501,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/a/build.log"),
        ),
        &base,
    );
    b1["upload_log_url"] = json!(format!("{base}/+files/b/build.log"));
    let mut b2 = rebase(
        ppa_build_json(502, "noble", "amd64", "Failed to build", true, None),
        &base,
    );
    b2["build_log_url"] =
        json!("https://user:supersecret@example.invalid/..%2fbad?tok=abc\u{202E}");

    let b1_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/501")
        .with_status(200)
        .with_body(b1.to_string())
        .create_async()
        .await;
    let log_a = server
        .mock("GET", "/+files/a/build.log")
        .with_status(200)
        .with_body(b"new-build-log")
        .create_async()
        .await;
    let log_b = server
        .mock("GET", "/+files/b/build.log")
        .with_status(200)
        .with_body(b"new-upload-log")
        .create_async()
        .await;

    // Pre-existing file with the same target name must stay intact; new log uses -2.
    std::fs::write(dir.path().join("build-501_build.log"), b"existing-log").unwrap();

    let client = mock_read_client(&server, true);
    let executed = execute_build_logs(
        &client,
        &BuildLogsRequest {
            target: BuildTarget::Single {
                build_id: Some(501),
                build_url: None,
                owner: None,
                ppa: Some("jdoe/test".into()),
                distro: "ubuntu".into(),
            },
            output_dir: dir.path().to_path_buf(),
            include_upload_logs: true,
        },
        lpcli::download::ProgressMode::None,
        |_| Ok(()),
    )
    .await
    .unwrap();
    build_logs_result(&executed).unwrap();

    assert_eq!(
        std::fs::read(dir.path().join("build-501_build.log")).unwrap(),
        b"existing-log"
    );
    assert_eq!(
        std::fs::read(dir.path().join("build-501_build-2.log")).unwrap(),
        b"new-build-log"
    );
    assert_eq!(
        std::fs::read(dir.path().join("build-501_upload_build.log")).unwrap(),
        b"new-upload-log"
    );

    // Unsafe URL with credentials, query, and bidi control characters must be redacted in report.
    let b2_mock = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/502")
        .with_status(200)
        .with_body(b2.to_string())
        .create_async()
        .await;
    let executed_bad = execute_build_logs(
        &client,
        &BuildLogsRequest {
            target: BuildTarget::Single {
                build_id: Some(502),
                build_url: None,
                owner: None,
                ppa: Some("jdoe/test".into()),
                distro: "ubuntu".into(),
            },
            output_dir: dir.path().to_path_buf(),
            include_upload_logs: false,
        },
        lpcli::download::ProgressMode::None,
        |_| Ok(()),
    )
    .await
    .unwrap();
    let bad_report = build_logs_report(&executed_bad);
    let dl_json = &bad_report.json["lpcli"]["downloads"][0];
    let shown_name = dl_json["filename"].as_str().unwrap();
    assert!(!shown_name.contains("supersecret"), "{shown_name}");
    assert!(!shown_name.contains("tok=abc"), "{shown_name}");
    assert!(!shown_name.contains('\u{202E}'), "{shown_name}");

    // `unique_backup_path` distinguishes upload_log and skips dangling symlinks.
    let mut used = std::collections::HashSet::new();
    let p_build = unique_backup_path(
        dir.path(),
        Some("700"),
        "build_log",
        false,
        "log.txt.gz",
        &mut used,
    );
    let p_upload = unique_backup_path(
        dir.path(),
        Some("700"),
        "upload_log",
        false,
        "log.txt.gz",
        &mut used,
    );
    assert_eq!(p_build.file_name().unwrap(), "build-700_log.txt.gz");
    assert_eq!(p_upload.file_name().unwrap(), "build-700_upload_log.txt.gz");

    let mut forced_used = std::collections::HashSet::new();
    let p_forced = unique_backup_path(
        dir.path(),
        Some("700"),
        "upload_log",
        true,
        "log.txt.gz",
        &mut forced_used,
    );
    assert_eq!(p_forced.file_name().unwrap(), "build-700_upload_log.txt.gz");

    #[cfg(unix)]
    {
        let mut sym_used = std::collections::HashSet::new();
        std::os::unix::fs::symlink(
            dir.path().join("missing"),
            dir.path().join("build-701_log.txt.gz"),
        )
        .unwrap();
        let p_sym = unique_backup_path(
            dir.path(),
            Some("701"),
            "build_log",
            false,
            "log.txt.gz",
            &mut sym_used,
        );
        assert_eq!(p_sym.file_name().unwrap(), "build-701_log-2.txt.gz");
    }

    b1_mock.assert_async().await;
    b2_mock.assert_async().await;
    log_a.assert_async().await;
    log_b.assert_async().await;
}

#[tokio::test]
async fn retry_save_logs_partial_exit_7_when_retry_post_fails_after_local_backup() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let dir = tempfile::tempdir().unwrap();

    let b = rebase(
        ppa_build_json(
            601,
            "noble",
            "amd64",
            "Failed to build",
            true,
            Some("/+files/b601.txt.gz"),
        ),
        &base,
    );
    let get_b = server
        .mock("GET", "/~jdoe/+archive/ubuntu/test/+build/601")
        .with_status(200)
        .with_body(b.to_string())
        .create_async()
        .await;
    let log_b = server
        .mock("GET", "/+files/b601.txt.gz")
        .with_status(200)
        .with_body(b"backed-up-before-403")
        .create_async()
        .await;
    let post_forbidden = server
        .mock("POST", "/~jdoe/+archive/ubuntu/test/+build/601")
        .with_status(403)
        .with_body("not allowed")
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let outcome = execute_build_retry(
        &client,
        false,
        &BuildRetryRequest {
            target: BuildTarget::Single {
                build_id: Some(601),
                build_url: None,
                owner: None,
                ppa: Some("jdoe/test".into()),
                distro: "ubuntu".into(),
            },
            dry_run: false,
            yes: true,
            save_logs: Some(dir.path().to_path_buf()),
        },
        |_| unreachable!(),
    )
    .await
    .unwrap();

    assert!(
        matches!(outcome.result, Err(LpError::Partial(_))),
        "local log backup + failed retry POST must yield Partial (exit 7): {:?}",
        outcome.result
    );
    assert_eq!(
        outcome.report.json["lpcli"]["saved_logs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        outcome.report.json["lpcli"]["failures"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read(dir.path().join("build-601_b601.txt.gz")).unwrap(),
        b"backed-up-before-403"
    );

    get_b.assert_async().await;
    log_b.assert_async().await;
    post_forbidden.assert_async().await;
}

#[tokio::test]
async fn fallback_build_resolution_deduplicates_and_package_builds_filters_pocket() {
    let mut server = isolated_mock_server().await;
    let base = server.url();

    let pub1 = json!({
        "self_link": format!("{base}/ubuntu/+archive/primary/+sourcepub/10"),
        "source_package_name": "mutter",
        "source_package_version": "50.1-1",
        "status": "Superseded",
        "pocket": "Updates",
        "distro_series_link": format!("{base}/ubuntu/noble"),
    });
    let pub2 = json!({
        "self_link": format!("{base}/ubuntu/+archive/primary/+sourcepub/11"),
        "source_package_name": "mutter",
        "source_package_version": "50.1-1",
        "status": "Published",
        "pocket": "Updates",
        "distro_series_link": format!("{base}/ubuntu/noble"),
    });

    let mut dup_build = rebase(
        ppa_build_json(701, "noble", "amd64", "Failed to build", true, None),
        &base,
    );
    dup_build["self_link"] = json!(format!("{base}/ubuntu/+archive/primary/+build/701"));
    dup_build["pocket"] = json!("Updates");

    let mut wrong_pocket_build = rebase(
        ppa_build_json(702, "noble", "amd64", "Failed to build", false, None),
        &base,
    );
    wrong_pocket_build["self_link"] = json!(format!("{base}/ubuntu/+archive/primary/+build/702"));
    wrong_pocket_build["pocket"] = json!("Release");

    let sources_mock = server
        .mock("GET", "/ubuntu/+archive/primary")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getPublishedSources".into()),
            Matcher::UrlEncoded("pocket".into(), "Updates".into()),
        ]))
        .with_status(200)
        .with_body(json!({"start": 0, "entries": [pub1, pub2]}).to_string())
        .expect(2)
        .create_async()
        .await;

    let builds_pub1 = server
        .mock("GET", "/ubuntu/+archive/primary/+sourcepub/10")
        .match_query(Matcher::UrlEncoded("ws.op".into(), "getBuilds".into()))
        .with_status(200)
        .with_body(json!({"start": 0, "entries": [dup_build, wrong_pocket_build]}).to_string())
        .expect(2)
        .create_async()
        .await;
    let builds_pub2 = server
        .mock("GET", "/ubuntu/+archive/primary/+sourcepub/11")
        .match_query(Matcher::UrlEncoded("ws.op".into(), "getBuilds".into()))
        .with_status(200)
        .with_body(json!({"start": 0, "entries": [dup_build]}).to_string())
        .expect(2)
        .create_async()
        .await;

    let post_701 = server
        .mock("POST", "/ubuntu/+archive/primary/+build/701")
        .match_body(Matcher::UrlEncoded("ws.op".into(), "retry".into()))
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let mut retried_701 = dup_build.clone();
    retried_701["buildstate"] = json!("Needs building");
    retried_701["can_be_retried"] = json!(false);
    let readback_701 = server
        .mock("GET", "/ubuntu/+archive/primary/+build/701")
        .with_status(200)
        .with_body(retried_701.to_string())
        .expect(1)
        .create_async()
        .await;

    let client = mock_read_client(&server, true);
    let outcome = execute_build_retry(
        &client,
        false,
        &BuildRetryRequest {
            target: BuildTarget::Filter(BuildFilterRequest {
                owner: None,
                ppa: None,
                name: Some("mutter".into()),
                version: None,
                series: Some("noble".into()),
                arch: Vec::new(),
                state: vec!["Failed to build".into()],
                exclude_state: Vec::new(),
                pocket: Some(ubuntu_info_rust::Pocket::Updates),
                distro: "ubuntu".into(),
                limit: 30,
            }),
            dry_run: false,
            yes: true,
            save_logs: None,
        },
        |_| unreachable!(),
    )
    .await
    .unwrap();
    outcome.result.unwrap();
    assert_eq!(outcome.retried_links.len(), 1);

    // `package_builds` must also filter out `Release` builds when `--pocket updates` is requested.
    let list_report = package_builds(
        &client,
        &PackageBuildsRequest {
            owner: None,
            ppa: None,
            name: Some("mutter".into()),
            version: None,
            series: Some("noble".into()),
            arch: Vec::new(),
            state: Vec::new(),
            exclude_state: Vec::new(),
            pocket: Some(ubuntu_info_rust::Pocket::Updates),
            distro: "ubuntu".into(),
            limit: 30,
        },
    )
    .await
    .unwrap();
    let visible_ids: Vec<&str> = list_report.json["builds"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["self_link"].as_str())
        .collect();
    assert!(
        visible_ids.iter().all(|s| s.ends_with("/+build/701")),
        "unexpected Release build included: {visible_ids:?}"
    );

    sources_mock.assert_async().await;
    builds_pub1.assert_async().await;
    builds_pub2.assert_async().await;
    post_701.assert_async().await;
    readback_701.assert_async().await;
}
