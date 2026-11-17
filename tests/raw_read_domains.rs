//! Raw read wrappers keep Launchpad's exact JSON for entries and collections.

use lpcli::client::{Collection, LaunchpadClient};
use lpcli::{cves, packages, people, projects, questions, specifications, translations};
use mockito::{Matcher, Server, ServerGuard};
use serde_json::{Value, json};

fn client(server: &ServerGuard) -> LaunchpadClient {
    LaunchpadClient::new(None).with_base_url(server.url())
}

async fn mock_json(server: &mut ServerGuard, path: &str, body: &Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_query(Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await
}

#[tokio::test]
async fn entry_raw_keeps_unknown_fields_nulls_and_timestamps() {
    let mut server = Server::new_async().await;
    let person = json!({
        "name": "jdoe",
        "display_name": "Jane Doe",
        "account_type": "person",
        "self_link": "https://api.launchpad.net/devel/~jdoe",
        "web_link": "https://launchpad.net/~jdoe",
        "resource_type_link": "https://api.launchpad.net/devel/#person",
        "http_etag": "\"abc-123\"",
        "date_created": "2006-01-02T03:04:05.123456+00:00",
        "preferred_email_address_link": null,
        "description": null,
        "karma": 42,
        "hide_email_addresses": true,
        "time_zone": null
    });
    let mock = mock_json(&mut server, "/~jdoe", &person).await;

    let raw = people::get_person_raw(&client(&server), "jdoe")
        .await
        .unwrap();

    assert_eq!(raw.raw, person);
    assert_eq!(raw.value.name, "jdoe");
    assert_eq!(raw.value.karma, Some(42));
    assert_eq!(
        raw.raw["date_created"], "2006-01-02T03:04:05.123456+00:00",
        "timestamp text must not be re-encoded"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn typed_wrappers_still_decode_the_same_entries() {
    let mut server = Server::new_async().await;
    let project = json!({
        "name": "launchpad",
        "display_name": "Launchpad",
        "summary": null,
        "description": null,
        "homepage_url": null,
        "self_link": null,
        "web_link": null,
        "active": true,
        "date_created": "2004-09-24T20:58:00.000000+00:00",
        "owner_link": null,
        "development_focus_link": null,
        "brand_new_field": [1, 2, 3]
    });
    let mock = server
        .mock("GET", "/launchpad")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(project.to_string())
        .expect(2)
        .create_async()
        .await;
    let client = client(&server);

    let typed = projects::get_project(&client, "launchpad").await.unwrap();
    let raw = projects::get_project_raw(&client, "launchpad")
        .await
        .unwrap();

    assert_eq!(typed, raw.value);
    assert_eq!(raw.raw["brand_new_field"], json!([1, 2, 3]));
    mock.assert_async().await;
}

#[tokio::test]
async fn fetch_all_raw_collections_keep_total_size_and_omit_next_link() {
    let mut server = Server::new_async().await;
    let base = server.url();
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
        .mock("GET", "/launchpad/all_milestones")
        .match_query(Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(first.to_string())
        .create_async()
        .await;
    let second_mock = server
        .mock("GET", "/launchpad/all_milestones")
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(second.to_string())
        .create_async()
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
    let mut server = Server::new_async().await;
    let total_size_link = format!(
        "{}/ubuntu/noble?ws.op=getTranslationTemplates&ws.show=total_size",
        server.url()
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
    let mock = mock_json(&mut server, "/ubuntu/noble", &page).await;

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
    let mut server = Server::new_async().await;
    let entry = json!({
        "name": "x", "display_name": "X", "sequence": "2024-0001", "id": 7,
        "unknown_field": null, "date_created": "2020-02-02T02:02:02.5+00:00"
    });
    let page = json!({"total_size": 1, "start": 0, "entries": [entry]});
    let _mock = server
        .mock("GET", Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .create_async()
        .await;
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
    let mut server = Server::new_async().await;
    let page = json!({
        "total_size": 5,
        "start": 0,
        "next_collection_link": format!("{}/ubuntu/+archive/primary?ws.start=2", server.url()),
        "entries": [publication("2.10-3"), publication("2.10-2")]
    });
    let mock = server
        .mock("GET", "/ubuntu/+archive/primary")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "getPublishedSources".into()),
            Matcher::UrlEncoded("ws.size".into(), "2".into()),
            Matcher::UrlEncoded("source_name".into(), "hello".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .expect(2)
        .create_async()
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
    assert_eq!(raw.value.total_size, Some(5));
    assert!(raw.value.next_collection_link.is_some());
    assert_eq!(typed, raw.value.entries);
    assert_eq!(typed.len(), 2);
    // Exactly two requests: one per call, never following the next page.
    mock.assert_async().await;
}

#[tokio::test]
async fn unlimited_ppa_source_listing_fetches_every_page() {
    let mut server = Server::new_async().await;
    let path = "/~jdoe/+archive/ubuntu/ppa";
    let first = json!({
        "total_size": 2,
        "start": 0,
        "next_collection_link": format!("{}{path}?ws.op=getPublishedSources&ws.start=1", server.url()),
        "entries": [publication("1")]
    });
    let second = json!({"start": 1, "entries": [publication("2")]});
    let first_mock = server
        .mock("GET", path)
        .match_query(Matcher::Exact("ws.op=getPublishedSources".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(first.to_string())
        .create_async()
        .await;
    let second_mock = server
        .mock("GET", path)
        .match_query(Matcher::UrlEncoded("ws.start".into(), "1".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(second.to_string())
        .create_async()
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
    let mut server = Server::new_async().await;
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
        .mock("GET", "/bugs/cve")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("ws.op".into(), "advancedSearch".into()),
            Matcher::UrlEncoded("limit".into(), "10".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(page.to_string())
        .expect(2)
        .create_async()
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
    mock.assert_async().await;
}

#[tokio::test]
async fn cve_search_is_incomplete_when_launchpad_hits_the_limit() {
    let mut server = Server::new_async().await;
    let page = json!({
        "total_size": 2,
        "start": 0,
        "entries": [cve("2024-0001", "a"), cve("2024-0002", "b")]
    });
    let _mock = mock_json(&mut server, "/bugs/cve", &page).await;
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
    let mut server = Server::new_async().await;
    let entry = json!({
        "name": "n", "display_name": null, "sequence": "2024-0001", "id": 9,
        "version": "1", "date_created": "2021-03-04T05:06:07.000001+00:00",
        "lp_only_field": {"a": null}
    });
    let _mock = server
        .mock("GET", Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(entry.to_string())
        .create_async()
        .await;
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
    let mut server = Server::new_async().await;
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
    let _bugs = mock_json(&mut server, "/bugs", &bug_tasks).await;
    let _messages = mock_json(&mut server, "/ubuntu/+question/9/messages", &messages).await;
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
