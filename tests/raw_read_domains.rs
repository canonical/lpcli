//! Raw read wrappers keep Launchpad's exact JSON for entries and collections.
//!
//! Tests that check lpcli against real Launchpad JSON replay recorded
//! fixtures from `tests/fixtures/raw_read_domains/` (see `support`); tests of
//! edge cases Launchpad rarely produces build their responses inline.

mod support;

use std::collections::BTreeSet;

use httpmock::{Method::GET, Mock, MockServer};
use lpcli::client::{Collection, LaunchpadClient};
use lpcli::{cves, packages, people, projects, questions, specifications, translations};
use serde::Serialize;
use serde_json::{Value, json};
use support::Fixture;

fn client(server: &MockServer) -> LaunchpadClient {
    LaunchpadClient::new(None).with_base_url(server.base_url())
}

/// Serve `body` for `GET path` with any query.
async fn mock_json<'a>(server: &'a MockServer, path: &str, body: &Value) -> Mock<'a> {
    server
        .mock_async(|when, then| {
            when.method(GET).path(path);
            then.status(200)
                .header("content-type", "application/json")
                .body(body.to_string());
        })
        .await
}

/// Serve `body` for every GET request.
async fn mock_any_json<'a>(server: &'a MockServer, body: &Value) -> Mock<'a> {
    server
        .mock_async(|when, then| {
            when.method(GET);
            then.status(200)
                .header("content-type", "application/json")
                .body(body.to_string());
        })
        .await
}

/// Top-level members of `raw` that the typed value does not model.
fn unmodelled_fields(raw: &Value, typed: &impl Serialize) -> BTreeSet<String> {
    let typed = serde_json::to_value(typed).expect("typed value serializes");
    raw.as_object()
        .expect("entry is a JSON object")
        .keys()
        .filter(|key| typed.get(key.as_str()).is_none())
        .cloned()
        .collect()
}

#[tokio::test]
async fn entry_raw_keeps_unknown_fields_nulls_and_timestamps() {
    let server = MockServer::start_async().await;
    let fixture = Fixture::load("raw_read_domains/team_launchpad", "/~launchpad").await;
    let team = fixture.json();
    let mock = fixture.serve(&server, "/~launchpad", |when| when).await;

    let raw = people::get_person_raw(&client(&server), "launchpad")
        .await
        .unwrap();

    assert_eq!(raw.raw, team);
    assert_eq!(raw.value.name, "launchpad");
    assert_eq!(raw.value.karma, team["karma"].as_i64());
    assert_eq!(
        raw.raw["date_created"], team["date_created"],
        "timestamp text must not be re-encoded"
    );
    // Keep the fixture meaningful across re-recordings: it must contain
    // nulls and members the typed `Person` does not model.
    assert!(team.as_object().unwrap().values().any(Value::is_null));
    assert!(!unmodelled_fields(&raw.raw, &raw.value).is_empty());
    mock.assert_async().await;
}

#[tokio::test]
async fn typed_wrappers_still_decode_the_same_entries() {
    let server = MockServer::start_async().await;
    let fixture = Fixture::load("raw_read_domains/project_launchpad", "/launchpad").await;
    let mock = fixture.serve(&server, "/launchpad", |when| when).await;
    let client = client(&server);

    let typed = projects::get_project(&client, "launchpad").await.unwrap();
    let raw = projects::get_project_raw(&client, "launchpad")
        .await
        .unwrap();

    assert_eq!(typed, raw.value);
    assert_eq!(raw.raw, fixture.json());
    let unmodelled = unmodelled_fields(&raw.raw, &raw.value);
    assert!(
        !unmodelled.is_empty(),
        "the raw entry keeps fields the typed Project drops"
    );
    mock.assert_calls_async(2).await;
}

#[tokio::test]
async fn fetch_all_raw_collections_keep_total_size_and_omit_next_link() {
    let server = MockServer::start_async().await;
    let base = server.base_url();
    let first = json!({
        "total_size": 2,
        "start": 0,
        "next_collection_link": format!("{base}/launchpad/all_milestones?ws.start=1&ws.size=1"),
        "entries": [{
            "name": "1.0",
            "title": null,
            "code_name": null,
            "is_active": false,
            "date_targeted": "2025-06-01",
            "self_link": null,
            "web_link": null,
            "target_link": null,
            "http_etag": "\"m1\""
        }]
    });
    let second = json!({
        "start": 1,
        "entries": [{
            "name": "2.0",
            "title": "Two",
            "code_name": null,
            "is_active": true,
            "date_targeted": null,
            "self_link": null,
            "web_link": null,
            "target_link": null,
            "extra": {"nested": null}
        }]
    });
    let first_mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/launchpad/all_milestones")
                .query_param_count(".*", ".*", 0);
            then.status(200)
                .header("content-type", "application/json")
                .body(first.to_string());
        })
        .await;
    let second_mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/launchpad/all_milestones")
                .query_param("ws.start", "1");
            then.status(200)
                .header("content-type", "application/json")
                .body(second.to_string());
        })
        .await;

    let raw = projects::list_milestones_raw(&client(&server), "launchpad")
        .await
        .unwrap();

    assert_eq!(
        raw.raw,
        json!({
            "total_size": 2,
            "start": 0,
            "entries": [first["entries"][0], second["entries"][0]],
        })
    );
    assert_eq!(
        raw.value
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["1.0", "2.0"]
    );
    first_mock.assert_async().await;
    second_mock.assert_async().await;
}

#[tokio::test]
async fn fetch_all_raw_collections_pass_total_size_link_through() {
    let server = MockServer::start_async().await;
    let total_size_link = format!(
        "{}/ubuntu/noble?ws.op=getTranslationTemplates&ws.show=total_size",
        server.base_url()
    );
    let page = json!({
        "total_size_link": total_size_link,
        "start": 0,
        "entries": [{
            "name": "firefox",
            "path": "po/firefox.pot",
            "priority": 100,
            "is_current": true,
            "is_obsolete": false,
            "date_last_updated": "2024-04-01T12:00:00+00:00",
            "self_link": null,
            "web_link": null,
            "iscurrent_extra": null
        }]
    });
    let mock = mock_json(&server, "/ubuntu/noble", &page).await;

    let raw = translations::get_distro_series_templates_raw(&client(&server), "ubuntu", "noble")
        .await
        .unwrap();

    assert_eq!(raw.raw, page);
    assert!(raw.raw.get("total_size").is_none());
    assert_eq!(raw.value.len(), 1);
    mock.assert_async().await;
}

#[tokio::test]
async fn every_collection_raw_wrapper_returns_launchpad_collection_shape() {
    let server = MockServer::start_async().await;
    let entry = json!({
        "name": "x", "display_name": "X", "sequence": "2024-0001", "id": 7,
        "unknown_field": null, "date_created": "2020-02-02T02:02:02.5+00:00"
    });
    let page = json!({"total_size": 1, "start": 0, "entries": [entry]});
    let _mock = mock_any_json(&server, &page).await;
    let client = client(&server);

    let raws: Vec<Value> = vec![
        people::search_people_raw(&client, "x").await.unwrap().raw,
        people::get_team_members_raw(&client, "t")
            .await
            .unwrap()
            .raw,
        people::list_person_ppas_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        people::get_person_owned_teams_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        projects::search_projects_raw(&client, "x")
            .await
            .unwrap()
            .raw,
        projects::list_active_milestones_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        projects::list_project_series_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        specifications::list_project_specifications_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        specifications::list_valid_project_specifications_raw(&client, "p")
            .await
            .unwrap()
            .raw,
        questions::search_questions_raw(&client, "p", &Default::default())
            .await
            .unwrap()
            .raw,
        cves::get_bug_cves_raw(&client, 1).await.unwrap().raw,
        packages::list_distro_series_raw(&client, "ubuntu")
            .await
            .unwrap()
            .raw,
        translations::get_distro_series_import_queue_raw(&client, "ubuntu", "noble")
            .await
            .unwrap()
            .raw,
    ];

    for raw in raws {
        assert_eq!(raw, page);
    }
}

fn publication(version: &str) -> Value {
    json!({
        "self_link": format!("https://api.launchpad.net/devel/ubuntu/+archive/primary/+sourcepub/{version}"),
        "source_package_name": "hello",
        "source_package_version": version,
        "component_name": "main",
        "section_name": null,
        "status": "Published",
        "date_published": "2024-04-25T10:11:12.987654+00:00",
        "date_superseded": null,
        "pocket": "Release",
        "archive_link": null,
        "distro_series_link": null,
        "display_name": "hello 2.10-3 in noble",
        "scheduled_deletion_date": null
    })
}

#[tokio::test]
async fn limited_source_search_fetches_one_page_and_keeps_next_link() {
    let server = MockServer::start_async().await;
    let fixture = Fixture::load(
        "raw_read_domains/hello_noble_sources_page",
        concat!(
            "/ubuntu/+archive/primary?ws.op=getPublishedSources",
            "&distro_series=https%3A%2F%2Fapi.launchpad.net%2Fdevel%2Fubuntu%2Fnoble",
            "&source_name=hello&ws.size=2",
        ),
    )
    .await;
    let page = fixture.json();
    let mock = fixture
        .serve(&server, "/ubuntu/+archive/primary", |when| {
            when.query_param("ws.op", "getPublishedSources")
                .query_param("ws.size", "2")
                .query_param("source_name", "hello")
        })
        .await;
    let client = client(&server);
    let params = packages::SourceSearchParams {
        source_name: Some("hello"),
        limit: Some(2),
        ..Default::default()
    };

    let raw = packages::search_published_sources_raw(&client, "ubuntu", "noble", &params)
        .await
        .unwrap();
    let typed = packages::search_published_sources(&client, "ubuntu", "noble", &params)
        .await
        .unwrap();

    assert_eq!(raw.raw, page);
    assert_eq!(raw.value.total_size, page["total_size"].as_u64());
    // The recorded page links to more results on api.launchpad.net.
    assert!(page["next_collection_link"].is_string());
    assert!(raw.value.next_collection_link.is_some());
    assert_eq!(typed, raw.value.entries);
    assert_eq!(typed.len(), 2);
    // Exactly two requests: one per call, never following the next page.
    mock.assert_calls_async(2).await;
}

#[tokio::test]
async fn unlimited_ppa_source_listing_fetches_every_page() {
    let server = MockServer::start_async().await;
    let path = "/~jdoe/+archive/ubuntu/ppa";
    let first = json!({
        "total_size": 2,
        "start": 0,
        "next_collection_link": format!("{}{path}?ws.op=getPublishedSources&ws.start=1", server.base_url()),
        "entries": [publication("1")]
    });
    let second = json!({"start": 1, "entries": [publication("2")]});
    let first_mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(path)
                .query_param("ws.op", "getPublishedSources")
                .query_param_count(".*", ".*", 1);
            then.status(200)
                .header("content-type", "application/json")
                .body(first.to_string());
        })
        .await;
    let second_mock = server
        .mock_async(|when, then| {
            when.method(GET).path(path).query_param("ws.start", "1");
            then.status(200)
                .header("content-type", "application/json")
                .body(second.to_string());
        })
        .await;

    let raw: lpcli::client::Raw<Collection<_>> =
        packages::list_ppa_sources_raw(&client(&server), "jdoe", "ppa", &Default::default())
            .await
            .unwrap();

    assert_eq!(
        raw.raw,
        json!({"total_size": 2, "start": 0, "entries": [publication("1"), publication("2")]})
    );
    assert!(raw.value.next_collection_link.is_none());
    assert_eq!(raw.value.total_size, Some(2));
    assert_eq!(raw.value.entries.len(), 2);
    first_mock.assert_async().await;
    second_mock.assert_async().await;
}

fn cve(sequence: &str, title: &str) -> Value {
    json!({
        "sequence": sequence,
        "status": "Entry",
        "description": null,
        "title": title,
        "url": null,
        "web_link": null,
        "self_link": null,
        "date_made_public": "2024-01-01T00:00:00+00:00",
        "cvss": {"nvd": "7.5"},
        "date_modified": null
    })
}

#[tokio::test]
async fn cve_search_filters_locally_and_keeps_exact_entries() {
    let server = MockServer::start_async().await;
    let page = json!({
        "total_size": 3,
        "start": 0,
        "entries": [
            cve("2024-0001", "OpenSSL overflow"),
            cve("2024-0002", "Kernel race"),
            cve("2024-0003", "openssl timing"),
        ]
    });
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/bugs/cve")
                .query_param("ws.op", "advancedSearch")
                .query_param("limit", "10");
            then.status(200)
                .header("content-type", "application/json")
                .body(page.to_string());
        })
        .await;
    let client = client(&server);
    let params = cves::CveSearchParams {
        limit: Some(10),
        search_text: Some("OPENSSL"),
        ..Default::default()
    };

    let found = cves::search_cves_raw(&client, &params).await.unwrap();
    let typed = cves::search_cves(&client, &params).await.unwrap();

    assert!(found.complete, "3 < limit 10, so every CVE was checked");
    assert_eq!(
        found.cves.iter().map(|c| c.raw.clone()).collect::<Vec<_>>(),
        [page["entries"][0].clone(), page["entries"][2].clone()]
    );
    assert_eq!(
        typed,
        found.cves.into_iter().map(|c| c.value).collect::<Vec<_>>()
    );
    mock.assert_calls_async(2).await;
}

#[tokio::test]
async fn cve_search_is_incomplete_when_launchpad_hits_the_limit() {
    let server = MockServer::start_async().await;
    let page = json!({
        "total_size": 2,
        "start": 0,
        "entries": [cve("2024-0001", "a"), cve("2024-0002", "b")]
    });
    let _mock = mock_json(&server, "/bugs/cve", &page).await;
    let client = client(&server);

    let limited = cves::search_cves_raw(
        &client,
        &cves::CveSearchParams {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let unlimited = cves::search_cves_raw(&client, &Default::default())
        .await
        .unwrap();

    assert_eq!(limited.cves.len(), 2);
    assert!(!limited.complete);
    assert!(
        !unlimited.complete,
        "Launchpad's default limit is unknown, so completeness cannot be proven"
    );
}

#[tokio::test]
async fn single_entry_raw_wrappers_return_exact_json() {
    let server = MockServer::start_async().await;
    let entry = json!({
        "name": "n", "display_name": null, "sequence": "2024-0001", "id": 9,
        "version": "1", "date_created": "2021-03-04T05:06:07.000001+00:00",
        "lp_only_field": {"a": null}
    });
    let _mock = mock_any_json(&server, &entry).await;
    let client = client(&server);

    let raws: Vec<Value> = vec![
        projects::get_milestone_raw(&client, "p", "m")
            .await
            .unwrap()
            .raw,
        projects::get_release_raw(&client, "p", "m")
            .await
            .unwrap()
            .raw,
        projects::get_project_series_raw(&client, "p", "s")
            .await
            .unwrap()
            .raw,
        specifications::get_specification_raw(&client, "p", "s")
            .await
            .unwrap()
            .raw,
        questions::get_question_by_id_raw(&client, 9)
            .await
            .unwrap()
            .raw,
        questions::get_question_raw(&client, "p", 9)
            .await
            .unwrap()
            .raw,
        cves::get_cve_raw(&client, "2024-0001").await.unwrap().raw,
        packages::get_distro_series_raw(&client, "ubuntu", "noble")
            .await
            .unwrap()
            .raw,
        packages::get_ppa_raw(&client, "o", "p").await.unwrap().raw,
        packages::get_distro_raw(&client, "ubuntu")
            .await
            .unwrap()
            .raw,
    ];

    for raw in raws {
        assert_eq!(raw, entry);
    }
}

#[tokio::test]
async fn person_bugs_and_question_messages_raw_keep_collection_metadata() {
    let server = MockServer::start_async().await;
    let bug_tasks = json!({
        "total_size_link": "https://api.launchpad.net/devel/bugs?ws.op=searchTasks&ws.show=total_size",
        "start": 0,
        "entries": []
    });
    let messages = json!({
        "total_size": 1,
        "start": 0,
        "entries": [{
            "index": 0, "content": "hi", "action": "Question", "owner_link": null,
            "date_created": "2022-02-02T02:02:02+00:00", "self_link": null,
            "web_link": null, "new_status": "Open"
        }]
    });
    let _bugs = mock_json(&server, "/bugs", &bug_tasks).await;
    let _messages = mock_json(&server, "/ubuntu/+question/9/messages", &messages).await;
    let client = client(&server);

    let bugs = people::get_person_bugs_raw(&client, "jdoe").await.unwrap();
    let msgs = questions::get_question_messages_raw(&client, "ubuntu", 9)
        .await
        .unwrap();

    assert_eq!(bugs.raw, bug_tasks);
    assert!(bugs.value.is_empty());
    assert_eq!(msgs.raw, messages);
    assert_eq!(msgs.value[0].action.as_deref(), Some("Question"));
}
