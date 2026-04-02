//! Low-level Launchpad API HTTP client.
//!
//! [`LaunchpadClient`] wraps a [`reqwest::Client`] and is responsible for:
//!
//! * Attaching OAuth 1.0a `Authorization` headers to every authenticated request.
//! * Sending GET and POST requests to the Launchpad REST API.
//! * Deserialising JSON responses into domain types.
//! * Translating HTTP errors into [`LpError`] values.
//!
//! The Launchpad REST API root is `https://api.launchpad.net/devel/`.
//!
//! # Example
//!
//! ```no_run
//! # use lpcli::client::LaunchpadClient;
//! # use lpcli::auth;
//! # tokio_test::block_on(async {
//! let creds = auth::load_credentials().unwrap();
//! let client = LaunchpadClient::new(Some(creds));
//! # });
//! ```

use std::collections::HashMap;

use reqwest::{StatusCode};
use serde::de::DeserializeOwned;
use crate::auth::{self, Credentials};
use crate::error::{LpError, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Base URL for the Launchpad devel API.
pub const API_BASE: &str = "https://api.launchpad.net/devel";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An authenticated (or unauthenticated) HTTP client for the Launchpad API.
#[derive(Debug, Clone)]
pub struct LaunchpadClient {
    http: reqwest::Client,
    credentials: Option<Credentials>,
    /// Override the API base URL (useful in tests).
    base_url: String,
}

impl LaunchpadClient {
    /// Create a new client.
    ///
    /// Pass `Some(creds)` for authenticated calls; `None` for anonymous access.
    pub fn new(credentials: Option<Credentials>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            credentials,
            base_url: API_BASE.to_string(),
        }
    }

    /// Override the base URL (used in integration tests with a mock server).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Resolve a relative path against the configured base URL.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Perform an authenticated GET request and deserialise the JSON body.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_url(&self.url(path)).await
    }

    /// Perform an authenticated GET request against an absolute URL.
    pub async fn get_url<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let mut req = self.http.get(url).header("Accept", "application/json");

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    /// Perform an authenticated POST request with a JSON body.
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<T> {
        let url = self.url(path);
        self.post_url(&url, params).await
    }

    /// Perform an authenticated POST request against an absolute URL.
    pub async fn post_url<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<T> {
        let mut req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .form(params);

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    /// Perform an authenticated PATCH request (used to update Launchpad resources).
    ///
    /// The Launchpad REST API requires `PATCH` bodies to be `application/json`.
    /// The `If-Match: *` header is included to satisfy Launchpad's optimistic
    /// concurrency check without requiring a prior `GET` to obtain an `ETag`.
    pub async fn patch_url<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<T> {
        let mut req = self
            .http
            .patch(url)
            .header("Accept", "application/json")
            .header("If-Match", "*")
            .json(params);

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    /// Perform an authenticated POST request and return `Ok(())` on success,
    /// discarding the response body.
    ///
    /// Use this for operations whose success response carries no JSON body
    /// (e.g. Launchpad `newMessage`, which returns `201 Created` with a
    /// `Location` header but an empty body).
    pub async fn post_ok(
        &self,
        path: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<()> {
        let url = self.url(path);
        let mut req = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .form(params);

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response_ok(resp).await
    }

    /// Perform an authenticated DELETE on an absolute URL, discarding the body.
    pub async fn delete_url_ok(&self, url: &str) -> Result<()> {
        let mut req = self
            .http
            .delete(url)
            .header("Accept", "application/json");

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response_ok(resp).await
    }

    /// POST with a slice of key-value pairs, allowing repeated keys (needed for
    /// Launchpad list parameters such as `event_types` and `scopes`).
    pub async fn post_pairs<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let url = self.url(path);
        self.post_pairs_url(&url, params).await
    }

    /// POST with key-value pairs against an absolute URL, returning a
    /// deserialised JSON body.
    pub async fn post_pairs_url<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let body = encode_pairs(params);
        let mut req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    /// POST with key-value pairs against a relative path, discarding the body.
    pub async fn post_pairs_ok(&self, path: &str, params: &[(&str, &str)]) -> Result<()> {
        let url = self.url(path);
        self.post_pairs_url_ok(&url, params).await
    }

    /// POST with key-value pairs against an absolute URL, discarding the body.
    pub async fn post_pairs_url_ok(&self, url: &str, params: &[(&str, &str)]) -> Result<()> {
        let body = encode_pairs(params);
        let mut req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        if let Some(creds) = &self.credentials {
            let auth_header = auth::build_auth_header(creds)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response_ok(resp).await
    }

    // -----------------------------------------------------------------------
    // Response handling
    // -----------------------------------------------------------------------

    async fn handle_response<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            // Read the response body so the exact Launchpad error message is
            // surfaced to the user rather than a generic string.
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            return Err(LpError::Api { status: 401, message: body });
        }
        if status == StatusCode::FORBIDDEN {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            return Err(LpError::Api {
                status: 403,
                message: format!(
                    "Permission denied. Verify your Launchpad account has the \
                     required permissions. {body}"
                ),
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(LpError::NotFound(
                "The requested resource does not exist on Launchpad.".to_string(),
            ));
        }
        if !status.is_success() {
            let code = status.as_u16();
            let message = resp.text().await.unwrap_or_else(|_| status.to_string());
            return Err(LpError::Api { status: code, message });
        }

        let bytes = resp.bytes().await?;
        serde_json::from_slice(&bytes).map_err(LpError::Json)
    }

    /// Like [`handle_response`] but discards the response body on success.
    ///
    /// Used for operations that return `201 Created` with no JSON body.
    async fn handle_response_ok(&self, resp: reqwest::Response) -> Result<()> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            return Err(LpError::Api { status: 401, message: body });
        }
        if status == StatusCode::FORBIDDEN {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            return Err(LpError::Api {
                status: 403,
                message: format!(
                    "Permission denied. Verify your Launchpad account has the \
                     required permissions. {body}"
                ),
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(LpError::NotFound(
                "The requested resource does not exist on Launchpad.".to_string(),
            ));
        }
        if !status.is_success() {
            let code = status.as_u16();
            let message = resp.text().await.unwrap_or_else(|_| status.to_string());
            return Err(LpError::Api { status: code, message });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Percent-encode a string for use as a URL path segment or query parameter value.
///
/// Exported so that library modules can produce safe URLs without a separate
/// dependency on `url::form_urlencoded`.
pub fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Encode a slice of key-value tuples as URL form-encoded data, allowing
/// repeated keys (which `HashMap` cannot represent).
fn encode_pairs(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| {
            let ek: String =
                url::form_urlencoded::byte_serialize(k.as_bytes()).collect();
            let ev: String =
                url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
            format!("{ek}={ev}")
        })
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------------
// Pagination helpers
// ---------------------------------------------------------------------------

/// A generic Launchpad collection response.
#[derive(Debug, serde::Deserialize)]
pub struct Collection<T> {
    /// The items in this page of results.
    pub entries: Vec<T>,
    /// URL of the next page, if any.
    pub next_collection_link: Option<String>,
    /// Total number of items across all pages.
    pub total_size: Option<u64>,
}

impl<T: DeserializeOwned + std::fmt::Debug> Collection<T> {
    /// Fetch all pages, returning the complete list of entries.
    ///
    /// Use this for queries where the full result set is required (e.g. listing
    /// all milestones or team members). For user-facing searches that include a
    /// `ws.size=N` page-size limit, use [`fetch_page`] instead to avoid
    /// exhausting all pages when only the first is needed.
    pub async fn fetch_all(
        client: &LaunchpadClient,
        first_url: &str,
    ) -> Result<Vec<T>> {
        let mut results = Vec::new();
        let mut url = first_url.to_string();
        loop {
            let page: Collection<T> = client.get_url(&url).await?;
            results.extend(page.entries);
            match page.next_collection_link {
                Some(next) => url = next,
                None => break,
            }
        }
        Ok(results)
    }

    /// Fetch a single page of results without following pagination links.
    ///
    /// Use this when the URL already contains a `ws.size=N` page-size limit
    /// (e.g. user-facing searches that should respect a `--limit` flag).
    /// Unlike [`fetch_all`], this makes exactly one HTTP request and returns
    /// only the entries on that page.
    pub async fn fetch_page(client: &LaunchpadClient, url: &str) -> Result<Vec<T>> {
        let page: Collection<T> = client.get_url(url).await?;
        Ok(page.entries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_builder_no_double_slash() {
        let client = LaunchpadClient::new(None);
        let url = client.url("/bugs/1");
        assert_eq!(url, "https://api.launchpad.net/devel/bugs/1");
    }

    #[test]
    fn url_builder_without_leading_slash() {
        let client = LaunchpadClient::new(None);
        let url = client.url("bugs/1");
        assert_eq!(url, "https://api.launchpad.net/devel/bugs/1");
    }

    #[test]
    fn with_base_url_overrides() {
        let client = LaunchpadClient::new(None).with_base_url("http://localhost:1234");
        let url = client.url("/bugs/1");
        assert_eq!(url, "http://localhost:1234/bugs/1");
    }

    #[test]
    fn client_is_clone() {
        let client = LaunchpadClient::new(None);
        let _cloned = client.clone();
    }

    /// Verify that unauthenticated clients have no credentials set.
    #[test]
    fn anonymous_client_has_no_credentials() {
        let client = LaunchpadClient::new(None);
        assert!(client.credentials.is_none());
    }

    /// Verify that an authenticated client stores its credentials.
    #[test]
    fn authenticated_client_stores_credentials() {
        use crate::auth::Credentials;
        let creds = Credentials::new("lpcli", "token", "secret");
        let client = LaunchpadClient::new(Some(creds.clone()));
        assert_eq!(client.credentials.as_ref().unwrap().access_token.token, "token");
    }
}
