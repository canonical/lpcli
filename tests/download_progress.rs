use std::num::NonZeroU64;

use lpcli::{client::LaunchpadClient, download::MilestoneProgress};

#[tokio::test]
async fn milestone_progress_works_with_the_authenticated_download_method_in_a_task() {
    let mut server = mockito::Server::new_async().await;
    let response = server
        .mock("GET", "/file")
        .with_body(b"contents")
        .create_async()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("file");
    let url = format!("{}/file", server.url());
    let client = LaunchpadClient::new(None).with_base_url(server.url());

    let reports = tokio::spawn(async move {
        let mut reports = Vec::new();
        let mut progress = MilestoneProgress::new(NonZeroU64::MAX, |bytes, total| {
            reports.push((bytes, total));
        });
        let bytes = client
            .download_to_file_with_progress(&url, &dest, progress.callback())
            .await
            .unwrap();
        progress.finish(bytes);
        reports
    })
    .await
    .unwrap();

    assert_eq!(reports, [(8, Some(8))]);
    assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), b"contents");
    response.assert_async().await;
}

#[tokio::test]
async fn milestone_progress_reports_chunked_and_empty_public_downloads() {
    let mut server = mockito::Server::new_async().await;
    let chunked = server
        .mock("GET", "/chunked")
        .with_chunked_body(|writer| writer.write_all(b"contents"))
        .create_async()
        .await;
    let empty = server
        .mock("GET", "/empty")
        .with_body(b"")
        .create_async()
        .await;
    let client = LaunchpadClient::new(None).with_base_url(server.url());
    let dir = tempfile::tempdir().unwrap();

    for (name, expected) in [
        ("chunked", b"contents".as_slice()),
        ("empty", b"".as_slice()),
    ] {
        let dest = dir.path().join(name);
        let mut reports = Vec::new();
        let mut progress = MilestoneProgress::new(NonZeroU64::MAX, |bytes, total| {
            reports.push((bytes, total));
        });
        let bytes = client
            .download_public_to_file_with_progress(
                &format!("{}/{name}", server.url()),
                &dest,
                progress.callback(),
            )
            .await
            .unwrap();
        progress.finish(bytes);

        assert_eq!(reports, [(expected.len() as u64, None)]);
        assert_eq!(std::fs::read(dest).unwrap(), expected);
    }

    chunked.assert_async().await;
    empty.assert_async().await;
}
