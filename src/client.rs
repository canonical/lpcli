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
            let auth_header =
                auth::build_auth_header("GET", url, creds, &HashMap::new())?;
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
            let auth_header =
                auth::build_auth_header("POST", url, creds, params)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    /// Perform an authenticated PATCH request (used to update Launchpad resources).
    pub async fn patch_url<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<T> {
        let mut req = self
            .http
            .patch(url)
            .header("Accept", "application/json")
            .form(params);

        if let Some(creds) = &self.credentials {
            let auth_header =
                auth::build_auth_header("PATCH", url, creds, params)?;
            req = req.header("Authorization", auth_header);
        }

        let resp = req.send().await?;
        self.handle_response(resp).await
    }

    // -----------------------------------------------------------------------
    // Response handling
    // -----------------------------------------------------------------------

    async fn handle_response<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(LpError::NotAuthenticated);
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
