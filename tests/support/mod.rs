//! Recorded Launchpad responses for the HTTP integration tests.
//!
//! A fixture is the JSON body Launchpad returned for one anonymous GET of a
//! public object, stored under `tests/fixtures/`. Tests serve it from a local
//! [`httpmock`] server, so the suite never contacts Launchpad.
//!
//! Re-record the fixtures the tests use with:
//!
//! ```sh
//! LPCLI_RECORD_FIXTURES=1 cargo test
//! ```
//!
//! Each fixture is then fetched again from `https://api.launchpad.net/devel`
//! without credentials and rewritten (pretty-printed, so re-recordings diff
//! cleanly) before the test runs against it. Review the diff before
//! committing: tests compare lpcli's output with the fixture rather than with
//! hard-coded values, but they assert properties a fixture must keep, such as
//! containing a `next_collection_link`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use httpmock::{Method::GET, Mock, MockServer};
use serde_json::Value;

/// Root of the live, anonymous Launchpad API that fixtures are recorded from.
const LIVE_API: &str = "https://api.launchpad.net/devel";

/// Set this environment variable to refresh fixtures from [`LIVE_API`].
/// An empty value or `0` leaves them as they are.
const RECORD_ENV: &str = "LPCLI_RECORD_FIXTURES";

/// How long recording one fixture may take.
const RECORD_TIMEOUT: Duration = Duration::from_secs(30);

/// A recorded Launchpad response body.
pub struct Fixture {
    body: String,
}

impl Fixture {
    /// Load `tests/fixtures/<name>.json`.
    ///
    /// When [`RECORD_ENV`] asks for it, first fetch `LIVE_API` + `api_path`
    /// (path and query, e.g. `/~launchpad`) and overwrite the fixture with
    /// the response.
    pub async fn load_or_record(name: &str, api_path: &str) -> Self {
        let path = fixture_path(name);
        if should_record_fixtures() {
            let body = fetch(api_path).await;
            record(&path, &body);
        }
        let body = read_fixture(&path);
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

/// The file that holds the fixture called `name`.
fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.json"))
}

/// Whether [`RECORD_ENV`] asks for the fixtures to be recorded.
fn should_record_fixtures() -> bool {
    records_fixtures(std::env::var_os(RECORD_ENV).as_deref())
}

/// Whether a value of [`RECORD_ENV`] asks for recording: set, not empty, and not `0`.
fn records_fixtures(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Read a recorded fixture, with a hint on how to record a missing one.
fn read_fixture(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "cannot read fixture {}: {err}; record it with {RECORD_ENV}=1",
            path.display()
        )
    })
}

/// Fetch `LIVE_API` + `api_path` anonymously and return the JSON body.
async fn fetch(api_path: &str) -> Value {
    let url = format!("{LIVE_API}{api_path}");
    let client = reqwest::Client::builder()
        // Launchpad rejects some anonymous requests that carry no User-Agent.
        .user_agent(concat!(
            "lpcli/",
            env!("CARGO_PKG_VERSION"),
            " (test fixture recorder)"
        ))
        .timeout(RECORD_TIMEOUT)
        .build()
        .expect("build the HTTP client");
    let response = client
        .get(&url)
        .header("accept", "application/json")
        .send()
        .await
        .unwrap_or_else(|err| panic!("cannot record {url}: {err}"));
    assert!(
        response.status().is_success(),
        "cannot record {url}: HTTP {}",
        response.status()
    );
    response
        .json()
        .await
        .unwrap_or_else(|err| panic!("{url} did not return JSON: {err}"))
}

/// Write `body` to `path`, pretty-printed so that re-recordings diff cleanly.
fn record(path: &Path, body: &Value) {
    let mut text = serde_json::to_string_pretty(body).expect("JSON serializes");
    text.push('\n');
    std::fs::create_dir_all(path.parent().expect("fixture has a directory"))
        .expect("create fixture directory");
    std::fs::write(path, text).expect("write fixture");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_fixtures_only_when_set_to_something_other_than_empty_or_zero() {
        assert!(!records_fixtures(None));
        assert!(!records_fixtures(Some(OsStr::new(""))));
        assert!(!records_fixtures(Some(OsStr::new("0"))));
        assert!(records_fixtures(Some(OsStr::new("1"))));
        assert!(records_fixtures(Some(OsStr::new("yes"))));
    }

    #[test]
    fn fixture_path_is_under_tests_fixtures() {
        let path = fixture_path("raw_read_domains/team_launchpad");
        assert!(path.ends_with("tests/fixtures/raw_read_domains/team_launchpad.json"));
    }
}
