//! Recorded Launchpad responses for the HTTP integration tests.
//!
//! A fixture is the JSON body Launchpad returned for one anonymous GET of a
//! public object, stored under `tests/fixtures/`. Tests serve it from a local
//! [`httpmock`] server, so the suite never contacts Launchpad.
//!
//! Re-record every fixture a test file uses with:
//!
//! ```sh
//! LPCLI_RECORD_FIXTURES=1 cargo test --test raw_read_domains
//! ```
//!
//! Each fixture is then fetched again from `https://api.launchpad.net/devel`
//! without credentials and rewritten (pretty-printed, so re-recordings diff
//! cleanly) before the test runs against it. Review the diff before
//! committing: tests compare lpcli's output with the fixture rather than with
//! hard-coded values, but they assert properties a fixture must keep, such as
//! containing a `next_collection_link`.

use std::path::{Path, PathBuf};

use httpmock::{Method::GET, Mock, MockServer};
use serde_json::Value;

/// Root of the live, anonymous Launchpad API that fixtures are recorded from.
const LIVE_API: &str = "https://api.launchpad.net/devel";

/// Set this environment variable to refresh fixtures from [`LIVE_API`].
const RECORD_ENV: &str = "LPCLI_RECORD_FIXTURES";

/// A recorded Launchpad response body.
pub struct Fixture {
    body: String,
}

impl Fixture {
    /// Load `tests/fixtures/<name>.json`.
    ///
    /// When [`RECORD_ENV`] is set, first fetch `LIVE_API` + `api_path` (path
    /// and query, e.g. `/~launchpad`) and overwrite the fixture with the
    /// response.
    pub async fn load(name: &str, api_path: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("{name}.json"));
        if std::env::var_os(RECORD_ENV).is_some() {
            record(&path, api_path).await;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "cannot read fixture {}: {err}; record it with {RECORD_ENV}=1",
                path.display()
            )
        });
        Self { body }
    }

    /// The fixture parsed as JSON, for comparing with lpcli's results.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("fixture is valid JSON")
    }

    /// Serve the fixture for `GET path` on `server`.
    ///
    /// Any query matches unless `when` adds query matchers. The returned mock
    /// counts the requests it answered.
    pub async fn serve<'a>(
        &self,
        server: &'a MockServer,
        path: &str,
        when: impl FnOnce(httpmock::When) -> httpmock::When,
    ) -> Mock<'a> {
        server
            .mock_async(|w, then| {
                when(w.method(GET).path(path));
                then.status(200)
                    .header("content-type", "application/json")
                    .body(&self.body);
            })
            .await
    }
}

/// Fetch `LIVE_API` + `api_path` anonymously and write the JSON body to `path`.
async fn record(path: &Path, api_path: &str) {
    let url = format!("{LIVE_API}{api_path}");
    let response = reqwest::Client::new()
        .get(&url)
        .header("accept", "application/json")
        // Launchpad rejects some anonymous requests that carry no User-Agent.
        .header(
            "user-agent",
            concat!(
                "lpcli/",
                env!("CARGO_PKG_VERSION"),
                " (test fixture recorder)"
            ),
        )
        .send()
        .await
        .unwrap_or_else(|err| panic!("cannot record {url}: {err}"));
    assert!(
        response.status().is_success(),
        "cannot record {url}: HTTP {}",
        response.status()
    );
    let body: Value = response
        .json()
        .await
        .unwrap_or_else(|err| panic!("{url} did not return JSON: {err}"));
    let mut text = serde_json::to_string_pretty(&body).expect("JSON serializes");
    text.push('\n');
    std::fs::create_dir_all(path.parent().expect("fixture has a directory"))
        .expect("create fixture directory");
    std::fs::write(path, text).expect("write fixture");
}
