//! Low-level Launchpad API HTTP client.
//!
//! [`LaunchpadClient`] wraps a [`reqwest::Client`] and is responsible for:
//!
//! * Attaching OAuth 1.0a `Authorization` headers to every authenticated request,
//!   using either stored credentials or short-lived access tokens minted from a
//!   hardware-bound grant (see [`crate::hardware_tokens`]).
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
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::auth::{self, Credentials};
use crate::error::{LpError, Result};
use crate::hardware_tokens::HardwareTokenSession;
use reqwest::{
    StatusCode,
    header::{HeaderMap, LOCATION, RETRY_AFTER},
};
use serde::de::DeserializeOwned;

fn api_http_client(hardware: bool, base_url: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .redirect(redirect_policy(hardware.then_some(base_url)))
        .build()
        .expect("Failed to build HTTP client")
}

fn redirect_policy(signing_origin: Option<&str>) -> reqwest::redirect::Policy {
    let Some(signing_origin) = signing_origin.map(str::to_owned) else {
        return reqwest::redirect::Policy::limited(10);
    };
    reqwest::redirect::Policy::custom(move |attempt| {
        let previous = attempt.previous();
        if previous.len() >= 10 {
            return attempt.error("too many redirects");
        }
        // Once a redirect leaves the signing origin, reqwest removes the
        // Authorization header. Do not follow a bounce back to that origin:
        // its eventual 401 would otherwise be mistaken for a signed request.
        if let Some(original) = previous.first()
            && same_origin(original.as_str(), &signing_origin)
            && same_origin(attempt.url().as_str(), original.as_str())
            && previous
                .iter()
                .any(|url| !same_origin(url.as_str(), original.as_str()))
        {
            return attempt.stop();
        }
        attempt.follow()
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Base URL for the Launchpad devel API.
pub const API_BASE: &str = "https://api.launchpad.net/devel";

// Private response metadata; never infer a rejected generation from the
// session's current credentials, which may already have changed.
#[derive(Clone)]
struct RejectedRequestCredentials(Credentials);

#[derive(Debug, Clone)]
enum HardwareSession {
    Real(Arc<HardwareTokenSession>),
    #[cfg(test)]
    Test(Arc<tests::TestHardwareSession>),
}

impl HardwareSession {
    async fn access_credentials(&self) -> Result<Credentials> {
        match self {
            Self::Real(session) => session.access_credentials().await,
            #[cfg(test)]
            Self::Test(session) => session.access_credentials().await,
        }
    }

    async fn invalidate_rejected_access_token(&self, credentials: &Credentials) -> Result<bool> {
        match self {
            Self::Real(session) => session.invalidate_rejected_access_token(credentials).await,
            #[cfg(test)]
            Self::Test(session) => session.invalidate_rejected_access_token(credentials).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An authenticated (or unauthenticated) HTTP client for the Launchpad API.
#[derive(Debug, Clone)]
pub struct LaunchpadClient {
    http: reqwest::Client,
    credentials: Option<Credentials>,
    /// A hardware-bound session; when set it signs requests instead of
    /// `credentials`.
    hardware: Option<HardwareSession>,
    /// Override the API base URL (useful in tests).
    base_url: String,
    /// Maximum number of retry attempts for 429 and 5xx responses.
    /// `0` means no retries (default).
    max_retries: u32,
    /// Initial delay between retry attempts in milliseconds.
    /// Doubles after each retry (exponential backoff).  Defaults to 1000 ms.
    initial_retry_delay_ms: u64,
}

impl LaunchpadClient {
    /// Create a new client.
    ///
    /// Pass `Some(creds)` for authenticated calls; `None` for anonymous access.
    pub fn new(credentials: Option<Credentials>) -> Self {
        Self {
            http: api_http_client(false, API_BASE),
            credentials,
            hardware: None,
            base_url: API_BASE.to_string(),
            max_retries: 0,
            initial_retry_delay_ms: 1_000,
        }
    }

    /// Create a client that signs requests with short-lived access tokens
    /// minted from a hardware-bound grant.
    ///
    /// The first signed request mints a token, which needs a touch of the
    /// security key; later requests reuse it until shortly before it
    /// expires.  Requests to hosts other than the API host are sent without
    /// an `Authorization` header. A redirect that leaves the signing origin
    /// and later returns to it is not followed: its request would no longer
    /// carry the original authorization, so a 401 could not identify the token.
    pub fn with_hardware_token(session: Arc<HardwareTokenSession>) -> Self {
        Self {
            http: api_http_client(true, API_BASE),
            hardware: Some(HardwareSession::Real(session)),
            ..Self::new(None)
        }
    }

    /// Whether this client signs requests with a hardware-bound session.
    pub fn is_hardware_bound(&self) -> bool {
        self.hardware.is_some()
    }

    /// Override the base URL (used in integration tests with a mock server).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        if self.hardware.is_some() {
            self.http = api_http_client(true, &self.base_url);
        }
        self
    }

    /// Set the maximum number of retry attempts for rate-limited (429) and
    /// server-error (5xx) responses.
    ///
    /// `0` (the default) disables retries. The backoff starts at
    /// `initial_retry_delay_ms` milliseconds, doubling after each attempt.
    /// For 429 responses, a valid `Retry-After` header (delay-seconds or
    /// HTTP-date) overrides the backoff, including a zero delay for past dates.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the initial backoff delay in milliseconds used between retries.
    ///
    /// Defaults to `1000` ms. The delay doubles after each failed attempt.
    pub fn with_retry_delay_ms(mut self, ms: u64) -> Self {
        self.initial_retry_delay_ms = ms;
        self
    }

    /// Resolve a relative path against the configured base URL.
    pub fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Perform an authenticated GET request and deserialise the JSON body.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_url(&self.url(path)).await
    }

    /// Perform an authenticated GET request against an absolute URL.
    ///
    /// With a hardware-bound session, a request rejected because its access
    /// token expired or was revoked is retried once with a new token.  Only
    /// safe (GET) requests are retried, since repeating others could
    /// duplicate their effects.
    pub async fn get_url<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        for attempt in 0..2 {
            let req = self.http.get(url).header("Accept", "application/json");
            let (req, credentials) = self.authorize_request(req, url).await?;
            let resp = self.send_with_retry(req).await?;
            let resp = self.attach_request_credentials(resp, credentials);
            let signed = resp
                .extensions()
                .get::<RejectedRequestCredentials>()
                .is_some();
            match self.handle_response(resp).await {
                Err(e) if attempt == 0 && signed && Self::hardware_token_rejected(&e) => continue,
                result => return result,
            }
        }
        unreachable!("the second GET attempt always returns")
    }

    /// Fetch a file resource URL (e.g. `diff_text_link`) and return the body
    /// as plain text.
    ///
    /// The Launchpad API `diff_text` endpoint rejects both anonymous and
    /// OAuth-authenticated requests.  Instead, the equivalent content is
    /// available via the web interface at `code.launchpad.net` which issues a
    /// 303 redirect to the librarian with an embedded download token.
    ///
    /// This method converts an API file-resource URL of the form:
    ///   `https://api.launchpad.net/devel/.../+preview-diff/ID/diff_text`
    /// to the corresponding web URL:
    ///   `https://code.launchpad.net/.../+preview-diff/ID/+files/preview.diff`
    /// and fetches that (following redirects).
    pub async fn get_text_url(&self, url: &str) -> Result<String> {
        // Convert API URL to web URL.
        if !url.starts_with("https://api.launchpad.net/devel/") || !url.ends_with("/diff_text") {
            return Err(LpError::Other(format!("unexpected diff_text URL: {url}")));
        }
        let web_url = url
            .replace(
                "https://api.launchpad.net/devel/",
                "https://code.launchpad.net/",
            )
            .replace("/diff_text", "/+files/preview.diff");

        let resp = self.send_with_retry(self.http.get(&web_url)).await?;

        if resp.status().is_success() {
            return resp.text().await.map_err(|e| LpError::Api {
                status: 0,
                message: format!("failed to read response: {e}"),
            });
        }

        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| "(could not read response body)".to_string());
        Err(LpError::Api {
            status,
            message: body,
        })
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
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .form(params);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response(resp).await
    }

    /// Perform an authenticated PATCH request (used to update Launchpad resources).
    ///
    /// The Launchpad REST API requires `PATCH` bodies to be `application/json`.
    /// No `If-Match` header is sent: Launchpad applies the change unconditionally
    /// when the header is absent.  Sending `If-Match: *` would cause Launchpad to
    /// respond with 412 because it treats the literal string `"*"` as an ETag
    /// value that never matches, rather than honouring the RFC wildcard semantics.
    pub async fn patch_url<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<T> {
        let req = self
            .http
            .patch(url)
            .header("Accept", "application/json")
            .json(params);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response(resp).await
    }

    /// Perform an authenticated PATCH request with an arbitrary JSON value body.
    ///
    /// Use this when the PATCH body contains non-string fields such as arrays
    /// (e.g. for updating a bug's `tags` field on the Launchpad API, which
    /// requires `{"tags": ["tag1", "tag2"]}` rather than a flat string map).
    ///
    /// The same `If-Match` caveat as [`Self::patch_url()`] applies.
    pub async fn patch_url_with_value<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let req = self
            .http
            .patch(url)
            .header("Accept", "application/json")
            .json(body);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response(resp).await
    }

    /// Perform an authenticated POST request and return `Ok(())` on success,
    /// discarding the response body.
    ///
    /// Use this for operations whose success response carries no JSON body
    /// (e.g. Launchpad `newMessage`, which returns `201 Created` with a
    /// `Location` header but an empty body).
    pub async fn post_ok(&self, path: &str, params: &HashMap<&str, &str>) -> Result<()> {
        let url = self.url(path);
        let req = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .form(params);

        let (req, credentials) = self.authorize_request(req, &url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response_ok(resp).await
    }

    /// Perform an authenticated POST request against an absolute URL and return
    /// `Ok(())` on success, discarding the response body.
    pub async fn post_url_ok(&self, url: &str, params: &HashMap<&str, &str>) -> Result<()> {
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .form(params);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response_ok(resp).await
    }

    /// Perform an authenticated POST request and return the `Location` header
    /// from a successful response.
    ///
    /// Use this for Launchpad operations that return `201 Created` with an
    /// empty body and provide the created resource URL in `Location`.
    pub async fn post_created_location(
        &self,
        path: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<String> {
        let url = self.url(path);
        self.post_url_created_location(&url, params).await
    }

    /// Perform an authenticated POST request against an absolute URL and
    /// return the `Location` header from a successful response.
    pub async fn post_url_created_location(
        &self,
        url: &str,
        params: &HashMap<&str, &str>,
    ) -> Result<String> {
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .form(params);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_created_location(resp).await
    }

    /// Perform an authenticated DELETE on an absolute URL, discarding the body.
    pub async fn delete_url_ok(&self, url: &str) -> Result<()> {
        let req = self.http.delete(url).header("Accept", "application/json");

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
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
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response(resp).await
    }

    /// POST with key-value pairs against a relative path, discarding the body.
    pub async fn post_pairs_ok(&self, path: &str, params: &[(&str, &str)]) -> Result<()> {
        let url = self.url(path);
        self.post_pairs_url_ok(&url, params).await
    }

    /// POST with key-value pairs against a relative path and return the
    /// `Location` header from a successful response.
    pub async fn post_pairs_created_location(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<String> {
        let url = self.url(path);
        self.post_pairs_url_created_location(&url, params).await
    }

    /// POST with key-value pairs against an absolute URL, discarding the body.
    pub async fn post_pairs_url_ok(&self, url: &str, params: &[(&str, &str)]) -> Result<()> {
        let body = encode_pairs(params);
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_response_ok(resp).await
    }

    /// POST with key-value pairs against an absolute URL and return the
    /// `Location` header from a successful response.
    pub async fn post_pairs_url_created_location(
        &self,
        url: &str,
        params: &[(&str, &str)],
    ) -> Result<String> {
        let body = encode_pairs(params);
        let req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_created_location(resp).await
    }

    /// Perform an authenticated form POST with repeatable keys and return
    /// the raw response, optionally sending a `Cookie` header.
    ///
    /// Used for multi-step operations whose server-side state lives in a
    /// session cookie, such as WebAuthn registration.
    pub(crate) async fn post_pairs_url_raw(
        &self,
        url: &str,
        params: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(encode_pairs(params));
        if let Some(cookie) = cookie {
            req = req.header(reqwest::header::COOKIE, cookie);
        }
        let (req, credentials) = self.authorize_request(req, url).await?;
        let resp = req.send().await?;
        Ok(self.attach_request_credentials(resp, credentials))
    }

    // -----------------------------------------------------------------------
    // Request signing
    // -----------------------------------------------------------------------

    /// Sign the request and retain the exact hardware credentials used for it.
    async fn authorize_request(
        &self,
        mut req: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<(reqwest::RequestBuilder, Option<Credentials>)> {
        let hardware_credentials = if let Some(session) = &self.hardware {
            if same_origin(url, &self.base_url) {
                Some(session.access_credentials().await?)
            } else {
                None
            }
        } else {
            None
        };
        let signing_credentials = if self.hardware.is_some() {
            hardware_credentials.as_ref()
        } else {
            self.credentials.as_ref()
        };
        if let Some(creds) = signing_credentials {
            req = req.header("Authorization", auth::build_auth_header(creds)?);
        }
        Ok((req, hardware_credentials))
    }

    /// Keep the signing identity with the response, including raw responses
    /// passed to the crate-visible response handlers by other modules.
    fn attach_request_credentials(
        &self,
        mut resp: reqwest::Response,
        credentials: Option<Credentials>,
    ) -> reqwest::Response {
        if resp.status() == StatusCode::UNAUTHORIZED
            && same_origin(resp.url().as_str(), &self.base_url)
            && let Some(credentials) = credentials
        {
            resp.extensions_mut()
                .insert(RejectedRequestCredentials(credentials));
        }
        resp
    }

    async fn note_unauthorized(&self, credentials: Option<RejectedRequestCredentials>, body: &str) {
        if let (Some(session), Some(credentials)) = (&self.hardware, credentials)
            && is_rejected_token_message(body)
        {
            // Local invalidation and blacklisting precede persistent cleanup.
            // A keyring failure must not hide Launchpad's rejection; explicit
            // cache-clearing operations still report their own errors.
            let _ = session
                .invalidate_rejected_access_token(&credentials.0)
                .await;
        }
    }

    /// Whether `error` reports a rejected hardware-bound access token.
    fn hardware_token_rejected(error: &LpError) -> bool {
        matches!(error, LpError::Api { status: 401, message }
            if is_rejected_token_message(message))
    }

    // -----------------------------------------------------------------------
    // Retry helper
    // -----------------------------------------------------------------------

    /// Send a request, retrying on 429 and 5xx responses up to `max_retries`
    /// times with exponential backoff.
    ///
    /// For 429 responses, `Retry-After` can specify delay-seconds or an
    /// HTTP-date. Future dates wait until that date; current/past dates do
    /// not wait. Missing, invalid or unrepresentable hints use the current
    /// backoff, which starts at `initial_retry_delay_ms` and doubles after
    /// each unsuccessful attempt. 5xx responses continue to use backoff.
    ///
    /// Returns the final `reqwest::Response` (which may still carry a 429 or
    /// 5xx status if all retries were exhausted).  Transport-level errors
    /// (`reqwest::Error`) are returned immediately without retry.
    async fn send_with_retry(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> reqwest::Result<reqwest::Response> {
        let mut delay_ms = self.initial_retry_delay_ms;
        let mut current = builder;
        let mut retries_left = self.max_retries;

        loop {
            // Clone the builder before sending so we can retry if needed.
            let (to_send, saved) = if retries_left > 0 {
                match current.try_clone() {
                    Some(clone) => (current, Some(clone)),
                    // Body is not cloneable (e.g. streaming); skip retry.
                    None => (current, None),
                }
            } else {
                (current, None)
            };

            let resp = to_send.send().await?;
            let status = resp.status();

            if let Some(next) = saved
                && (status.as_u16() == 429 || status.is_server_error())
            {
                let sleep_for = calculate_retry_delay(
                    status,
                    resp.headers(),
                    Duration::from_millis(delay_ms),
                    SystemTime::now(),
                );
                tokio::time::sleep(sleep_for).await;
                delay_ms = delay_ms.saturating_mul(2);
                retries_left -= 1;
                current = next;
                continue;
            }

            return Ok(resp);
        }
    }

    // -----------------------------------------------------------------------
    // Response handling
    // -----------------------------------------------------------------------

    pub(crate) async fn handle_response<T: DeserializeOwned>(
        &self,
        mut resp: reqwest::Response,
    ) -> Result<T> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            // Read the response body so the exact Launchpad error message is
            // surfaced to the user rather than a generic string.
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            self.note_unauthorized(credentials, &body).await;
            return Err(LpError::Api {
                status: 401,
                message: body,
            });
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
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after_seconds(resp.headers(), SystemTime::now());
            return Err(LpError::RateLimit {
                retry_after_secs: retry_after,
            });
        }
        if status == StatusCode::PRECONDITION_FAILED {
            return Err(LpError::Api {
                status: 412,
                message: "Precondition failed: the resource was modified by another \
                          client since it was last fetched. Retry the operation to \
                          apply your change to the current version."
                    .to_string(),
            });
        }
        if !status.is_success() {
            let code = status.as_u16();
            let message = resp.text().await.unwrap_or_else(|_| status.to_string());
            return Err(LpError::Api {
                status: code,
                message,
            });
        }

        let bytes = resp.bytes().await?;
        serde_json::from_slice(&bytes).map_err(LpError::Json)
    }

    /// Like [`handle_response`] but discards the response body on success.
    ///
    /// Used for operations that return `201 Created` with no JSON body.
    async fn handle_response_ok(&self, mut resp: reqwest::Response) -> Result<()> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            self.note_unauthorized(credentials, &body).await;
            return Err(LpError::Api {
                status: 401,
                message: body,
            });
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
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after_seconds(resp.headers(), SystemTime::now());
            return Err(LpError::RateLimit {
                retry_after_secs: retry_after,
            });
        }
        if status == StatusCode::PRECONDITION_FAILED {
            return Err(LpError::Api {
                status: 412,
                message: "Precondition failed: the resource was modified by another \
                          client since it was last fetched. Retry the operation to \
                          apply your change to the current version."
                    .to_string(),
            });
        }
        if !status.is_success() {
            let code = status.as_u16();
            let message = resp.text().await.unwrap_or_else(|_| status.to_string());
            return Err(LpError::Api {
                status: code,
                message,
            });
        }
        Ok(())
    }

    /// Like [`handle_response_ok`] but returns the `Location` header value on
    /// success.
    pub(crate) async fn handle_created_location(
        &self,
        mut resp: reqwest::Response,
    ) -> Result<String> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read response body)".to_string());
            self.note_unauthorized(credentials, &body).await;
            return Err(LpError::Api {
                status: 401,
                message: body,
            });
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
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after_seconds(resp.headers(), SystemTime::now());
            return Err(LpError::RateLimit {
                retry_after_secs: retry_after,
            });
        }
        if !status.is_success() {
            let code = status.as_u16();
            let message = resp.text().await.unwrap_or_else(|_| status.to_string());
            return Err(LpError::Api {
                status: code,
                message,
            });
        }

        let location = resp
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                LpError::Other(
                    "Launchpad response did not include a valid Location header".to_string(),
                )
            })?;

        Ok(location.to_string())
    }

    // -----------------------------------------------------------------------
    // File download
    // -----------------------------------------------------------------------

    /// Download a file from `url` and write it to `dest`, returning the number
    /// of bytes written.
    ///
    /// The request is authenticated when credentials are present on the client.
    /// Launchpad librarian URLs (which are typically public) will be followed
    /// through redirects automatically by the underlying HTTP client.
    ///
    /// This method streams the response body to disk in chunks rather than
    /// buffering the entire file in memory, which avoids timeout errors on
    /// large files.
    pub async fn download_to_file(&self, url: &str, dest: &std::path::Path) -> Result<u64> {
        use tokio::io::AsyncWriteExt;

        // Build a separate client without the default 30s timeout so that
        // large file downloads are not interrupted.  We keep a connect
        // timeout to fail fast on unreachable hosts.
        let download_client = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(300))
            .redirect(redirect_policy(
                self.hardware.is_some().then_some(&self.base_url),
            ))
            .build()
            .map_err(|e| LpError::Other(format!("Failed to build download client: {e}")))?;

        let req = download_client.get(url);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                LpError::Timeout(format!("Connection timed out downloading {url}: {e}"))
            } else if e.is_connect() {
                LpError::Connect(format!("Connection failed downloading {url}: {e}"))
            } else {
                LpError::Http(e)
            }
        })?;

        let mut resp = self.attach_request_credentials(resp, credentials);
        let status = resp.status();

        if !status.is_success() {
            let code = status.as_u16();
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read body)".to_string());
            self.note_unauthorized(credentials, &body).await;
            return Err(LpError::Api {
                status: code,
                message: format!("Failed to download {url}: HTTP {code} — {body}"),
            });
        }

        // Stream response body to a file in chunks.
        let mut file = tokio::fs::File::create(dest).await.map_err(|e| {
            LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to create file '{}': {e}", dest.display()),
            ))
        })?;

        let mut total_bytes: u64 = 0;
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                LpError::Other(format!(
                    "Network error while downloading '{}': {e}",
                    dest.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| url.to_string()),
                ))
            })?;
            file.write_all(&chunk).await.map_err(|e| {
                LpError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to write to '{}': {e}", dest.display()),
                ))
            })?;
            total_bytes += chunk.len() as u64;
        }

        file.flush().await.map_err(|e| {
            LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to flush file '{}': {e}", dest.display()),
            ))
        })?;

        Ok(total_bytes)
    }

    /// Download a file from `url` into `dest`, reporting progress via a callback.
    ///
    /// The callback `on_progress` is invoked with `(bytes_downloaded_so_far, total_size)`.
    /// `total_size` is `None` if the server did not provide a `Content-Length` header.
    ///
    /// Returns the total number of bytes written to `dest`.
    pub async fn download_to_file_with_progress<F>(
        &self,
        url: &str,
        dest: &std::path::Path,
        on_progress: F,
    ) -> Result<u64>
    where
        F: Fn(u64, Option<u64>),
    {
        use tokio::io::AsyncWriteExt;

        let download_client = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(300))
            .redirect(redirect_policy(
                self.hardware.is_some().then_some(&self.base_url),
            ))
            .build()
            .map_err(|e| LpError::Other(format!("Failed to build download client: {e}")))?;

        let req = download_client.get(url);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                LpError::Timeout(format!("Connection timed out downloading {url}: {e}"))
            } else if e.is_connect() {
                LpError::Connect(format!("Connection failed downloading {url}: {e}"))
            } else {
                LpError::Http(e)
            }
        })?;

        let mut resp = self.attach_request_credentials(resp, credentials);
        let status = resp.status();

        if !status.is_success() {
            let code = status.as_u16();
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read body)".to_string());
            self.note_unauthorized(credentials, &body).await;
            return Err(LpError::Api {
                status: code,
                message: format!("Failed to download {url}: HTTP {code} — {body}"),
            });
        }

        let content_length = resp.content_length();

        let mut file = tokio::fs::File::create(dest).await.map_err(|e| {
            LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to create file '{}': {e}", dest.display()),
            ))
        })?;

        let mut total_bytes: u64 = 0;
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                LpError::Other(format!(
                    "Network error while downloading '{}': {e}",
                    dest.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| url.to_string()),
                ))
            })?;
            file.write_all(&chunk).await.map_err(|e| {
                LpError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to write to '{}': {e}", dest.display()),
                ))
            })?;
            total_bytes += chunk.len() as u64;
            on_progress(total_bytes, content_length);
        }

        file.flush().await.map_err(|e| {
            LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to flush file '{}': {e}", dest.display()),
            ))
        })?;

        Ok(total_bytes)
    }

    /// Perform an authenticated GET against an absolute URL and return the
    /// JSON response body as a `Vec<String>`.
    ///
    /// Use this for Launchpad custom GET methods that return a JSON list of
    /// strings (e.g. `sourceFileUrls`, `binaryFileUrls`).
    pub async fn get_url_string_list(&self, url: &str) -> Result<Vec<String>> {
        self.get_url(url).await
    }

    /// Perform an authenticated multipart POST and return the `Location`
    /// header from a `201 Created` response.
    ///
    /// Used for Launchpad operations that accept file uploads (e.g.
    /// `addAttachment`).
    pub async fn post_multipart_created_location(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<String> {
        let url = self.url(path);
        let req = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .multipart(form);

        let (req, credentials) = self.authorize_request(req, &url).await?;

        let resp = self.send_with_retry(req).await?;
        let resp = self.attach_request_credentials(resp, credentials);
        self.handle_created_location(resp).await
    }

    /// Resolve a redirect URL without following it.
    ///
    /// Sends an authenticated GET with redirect policy set to `none` and
    /// returns the `Location` header value from a 3xx response.  Returns
    /// `None` if the response is not a redirect or lacks a `Location` header.
    /// A rejected hardware-bound access token returns the original 401 error.
    pub async fn resolve_redirect(&self, url: &str) -> Result<Option<String>> {
        let no_redirect_client = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| LpError::Other(format!("Failed to build HTTP client: {e}")))?;

        let req = no_redirect_client
            .get(url)
            .header("Accept", "application/json");

        let (req, credentials) = self.authorize_request(req, url).await?;

        let resp = req.send().await?;
        let mut resp = self.attach_request_credentials(resp, credentials);

        if resp.status() == StatusCode::UNAUTHORIZED {
            let credentials = resp.extensions_mut().remove::<RejectedRequestCredentials>();
            let body = resp.text().await.unwrap_or_default();
            let rejected = credentials.is_some() && is_rejected_token_message(&body);
            self.note_unauthorized(credentials, &body).await;
            return if rejected {
                Err(LpError::Api {
                    status: 401,
                    message: body,
                })
            } else {
                Ok(None)
            };
        }
        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            Ok(location)
        } else {
            Ok(None)
        }
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

/// Parse RFC 9110 Retry-After advice, using an explicit clock for date math.
/// Unrepresentable delays are rejected rather than overflowing a deadline.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim_matches([' ', '\t']);
    let delay = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        Duration::from_secs(value.parse().ok()?)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .unwrap_or(Duration::ZERO)
    };
    now.checked_add(delay)?;
    Some(delay)
}

/// Extract and parse the `Retry-After` header from `headers` into a wait
/// [`Duration`] relative to `now`.
///
/// Returns `None` if the header is missing, contains non-ASCII bytes, or does
/// not contain a valid, representable RFC 9110 `Retry-After` value.
fn parse_retry_after_header(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    parse_retry_after(headers.get(RETRY_AFTER)?.to_str().ok()?, now)
}

/// Calculate the duration to wait before the next retry attempt.
///
/// For `429 Too Many Requests` responses, a valid `Retry-After` header delay
/// that can be scheduled by the Tokio timer takes precedence over `backoff`.
/// All other retryable statuses (such as `5xx` server errors), or responses
/// without a usable `Retry-After` header, fall back to `backoff`.
fn calculate_retry_delay(
    status: StatusCode,
    headers: &HeaderMap,
    backoff: Duration,
    now: SystemTime,
) -> Duration {
    if status == StatusCode::TOO_MANY_REQUESTS {
        parse_retry_after_header(headers, now)
            .filter(|delay| tokio::time::Instant::now().checked_add(*delay).is_some())
            .unwrap_or(backoff)
    } else {
        backoff
    }
}

/// RateLimit exposes whole seconds; round up so its advice never retries early.
fn retry_after_seconds(headers: &HeaderMap, now: SystemTime) -> Option<u64> {
    parse_retry_after_header(headers, now).map(|delay| {
        delay
            .as_secs()
            .saturating_add(u64::from(delay.subsec_nanos() != 0))
    })
}

/// Whether a 401 response body says that the access token itself was
/// rejected (as opposed to, say, a missing permission).
fn is_rejected_token_message(body: &str) -> bool {
    body.starts_with("Expired token") || body.starts_with("Unknown access token")
}

/// Whether `url` has the same scheme, host and port as `base`.
fn same_origin(url: &str, base: &str) -> bool {
    match (url::Url::parse(url), url::Url::parse(base)) {
        (Ok(a), Ok(b)) => {
            a.scheme() == b.scheme()
                && a.host_str() == b.host_str()
                && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

/// Encode a slice of key-value tuples as URL form-encoded data, allowing
/// repeated keys (which `HashMap` cannot represent).
fn encode_pairs(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| {
            let ek: String = url::form_urlencoded::byte_serialize(k.as_bytes()).collect();
            let ev: String = url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
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
    /// `ws.size=N` page-size limit, use [`Self::fetch_page()`] instead to avoid
    /// exhausting all pages when only the first is needed.
    pub async fn fetch_all(client: &LaunchpadClient, first_url: &str) -> Result<Vec<T>> {
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
    /// Unlike [`Self::fetch_all()`], this makes exactly one HTTP request and returns
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
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug, Default)]
    struct TestSessionState {
        current: Option<Credentials>,
        minted: usize,
        rejected: Vec<Credentials>,
    }

    #[derive(Debug, Default)]
    pub(super) struct TestHardwareSession {
        state: tokio::sync::Mutex<TestSessionState>,
        fail_invalidation: AtomicBool,
    }

    impl TestHardwareSession {
        pub(super) async fn access_credentials(&self) -> Result<Credentials> {
            let mut state = self.state.lock().await;
            if let Some(credentials) = &state.current {
                return Ok(credentials.clone());
            }
            state.minted += 1;
            let credentials =
                Credentials::new("test-consumer", format!("token{}", state.minted), "secret");
            state.current = Some(credentials.clone());
            Ok(credentials)
        }

        pub(super) async fn invalidate_rejected_access_token(
            &self,
            credentials: &Credentials,
        ) -> Result<bool> {
            let mut state = self.state.lock().await;
            state.rejected.push(credentials.clone());
            let matches = state.current.as_ref() == Some(credentials);
            if matches {
                state.current = None;
            }
            if self.fail_invalidation.load(Ordering::SeqCst) {
                return Err(LpError::Keyring("test deletion failure".into()));
            }
            Ok(matches)
        }
    }

    fn test_hardware_client(base: &str) -> (LaunchpadClient, Arc<TestHardwareSession>) {
        let session = Arc::new(TestHardwareSession::default());
        let mut client = LaunchpadClient::new(None).with_base_url(base);
        client.http = api_http_client(true, base);
        client.hardware = Some(HardwareSession::Test(session.clone()));
        (client, session)
    }

    fn token_header(token: &str) -> mockito::Matcher {
        mockito::Matcher::Regex(format!(r#"oauth_token="{token}""#))
    }

    const HTTP_DATE: &str = "Sun, 06 Nov 1994 08:49:37 GMT";

    fn date() -> SystemTime {
        httpdate::parse_http_date(HTTP_DATE).unwrap()
    }

    fn retry_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, value.parse().unwrap());
        headers
    }

    #[test]
    fn retry_after_parses_integer_seconds() {
        for (value, secs) in [("0", 0), ("120", 120), ("0012", 12), (" \t12\t ", 12)] {
            assert_eq!(
                parse_retry_after(value, date()),
                Some(Duration::from_secs(secs))
            );
        }
    }

    #[test]
    fn retry_after_accepts_all_http_date_formats() {
        let now = date() - Duration::from_secs(120);
        for value in [
            HTTP_DATE,
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(
                parse_retry_after(value, now),
                Some(Duration::from_secs(120))
            );
        }
        assert_eq!(
            parse_retry_after(&format!(" \t{HTTP_DATE}\t "), now),
            Some(Duration::from_secs(120))
        );
    }

    #[test]
    fn retry_after_past_and_current_dates_are_zero() {
        assert_eq!(parse_retry_after(HTTP_DATE, date()), Some(Duration::ZERO));
        assert_eq!(
            parse_retry_after(HTTP_DATE, date() + Duration::from_secs(1)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn retry_after_preserves_fractional_delay_and_rounds_up_for_errors() {
        let now = date() - Duration::from_millis(1250);
        let headers = retry_headers(HTTP_DATE);
        assert_eq!(
            parse_retry_after_header(&headers, now),
            Some(Duration::from_millis(1250))
        );
        assert_eq!(retry_after_seconds(&headers, now), Some(2));
        assert_eq!(retry_after_seconds(&headers, date()), Some(0));
        assert_eq!(retry_after_seconds(&retry_headers("120"), now), Some(120));
        assert_eq!(
            retry_after_seconds(&headers, date() - Duration::from_nanos(1)),
            Some(1)
        );
    }

    #[test]
    fn retry_after_rejects_invalid_and_unrepresentable_values() {
        for value in [
            "",
            " ",
            "-1",
            "+1",
            "1.5",
            "١٢",
            "12 seconds",
            "not a date",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "18446744073709551616",
            "18446744073709551615",
        ] {
            assert_eq!(parse_retry_after(value, date()), None, "{value}");
        }
        assert_eq!(retry_after_seconds(&HeaderMap::new(), date()), None);
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            reqwest::header::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        assert_eq!(parse_retry_after_header(&headers, date()), None);
    }

    #[test]
    fn calculate_retry_delay_uses_backoff_only_without_usable_429_advice() {
        let backoff = Duration::from_millis(100);
        for headers in [
            HeaderMap::new(),
            retry_headers("invalid"),
            retry_headers("18446744073709551615"),
        ] {
            assert_eq!(
                calculate_retry_delay(StatusCode::TOO_MANY_REQUESTS, &headers, backoff, date()),
                backoff
            );
        }
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(HTTP_DATE),
                backoff,
                date()
            ),
            Duration::ZERO
        );
        // HTTP-date parsing does not broaden which statuses honor Retry-After.
        assert_eq!(
            calculate_retry_delay(
                StatusCode::SERVICE_UNAVAILABLE,
                &retry_headers("0"),
                backoff,
                date()
            ),
            backoff
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_sleep_preserves_server_delay() {
        // Exercise the same delay selection and timer used by send_with_retry,
        // without real socket I/O or a wall clock that moves during the test.
        for (header, delay) in [
            ("2", Duration::from_secs(2)),
            (HTTP_DATE, Duration::from_millis(1250)),
        ] {
            let selected = calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(header),
                Duration::from_millis(1),
                date() - Duration::from_millis(1250),
            );
            assert_eq!(selected, delay);
            let sleep = tokio::time::sleep(selected);
            tokio::pin!(sleep);
            assert!(futures_util::poll!(sleep.as_mut()).is_pending());
            tokio::time::advance(delay - Duration::from_millis(1)).await;
            assert!(futures_util::poll!(sleep.as_mut()).is_pending());
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(futures_util::poll!(sleep.as_mut()).is_ready());
        }
    }

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

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        let base = "https://api.launchpad.net/devel";
        assert!(same_origin("https://api.launchpad.net/devel/~me", base));
        assert!(same_origin("https://api.launchpad.net:443/devel", base));
        assert!(!same_origin("https://launchpadlibrarian.net/1/file", base));
        assert!(!same_origin("http://api.launchpad.net/devel", base));
        assert!(!same_origin("not a url", base));
    }

    #[test]
    fn rejected_token_messages() {
        assert!(is_rejected_token_message("Expired token (abc)"));
        assert!(is_rejected_token_message("Unknown access token (abc)."));
        assert!(!is_rejected_token_message(
            "Unauthorized: missing permission"
        ));
    }

    #[tokio::test]
    async fn signed_get_retries_once_and_invalidates_each_rejected_generation() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Expired token (token1)")
            .create_async()
            .await;
        let second = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token2"))
            .with_status(401)
            .with_body("Unknown access token (token2)")
            .create_async()
            .await;
        let (client, session) = test_hardware_client(&server.url());
        let result: Result<serde_json::Value> = client.get("item").await;
        assert!(matches!(result, Err(LpError::Api { status: 401, .. })));
        first.assert_async().await;
        second.assert_async().await;
        let state = session.state.lock().await;
        assert_eq!(state.minted, 2);
        assert_eq!(
            state
                .rejected
                .iter()
                .map(|c| c.access_token.token.as_str())
                .collect::<Vec<_>>(),
            ["token1", "token2"]
        );
        assert!(state.current.is_none());
    }

    #[tokio::test]
    async fn get_recovers_after_persistent_invalidation_failure() {
        let mut server = mockito::Server::new_async().await;
        let rejected = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let fresh = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token2"))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let (client, session) = test_hardware_client(&server.url());
        session.fail_invalidation.store(true, Ordering::SeqCst);
        let _: serde_json::Value = client.get("item").await.unwrap();
        rejected.assert_async().await;
        fresh.assert_async().await;
        let state = session.state.lock().await;
        assert_eq!(state.minted, 2);
        assert_eq!(state.rejected[0].access_token.token, "token1");
        assert_eq!(state.current.as_ref().unwrap().access_token.token, "token2");
    }

    #[tokio::test]
    async fn get_and_mutation_rejections_survive_invalidation_failure() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let second = server
            .mock("GET", "/item")
            .match_header("authorization", token_header("token2"))
            .with_status(401)
            .with_body("Unknown access token")
            .create_async()
            .await;
        let mutation = server
            .mock("POST", "/item")
            .match_header("authorization", token_header("token3"))
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let (client, session) = test_hardware_client(&server.url());
        session.fail_invalidation.store(true, Ordering::SeqCst);
        let result: Result<serde_json::Value> = client.get("item").await;
        assert!(
            matches!(result, Err(LpError::Api { status: 401, message }) if message == "Unknown access token")
        );
        let result: Result<serde_json::Value> = client.post("item", &HashMap::new()).await;
        assert!(
            matches!(result, Err(LpError::Api { status: 401, message }) if message == "Expired token")
        );
        for mock in [first, second, mutation] {
            mock.assert_async().await;
        }
        let state = session.state.lock().await;
        assert_eq!(state.minted, 3);
        assert_eq!(state.rejected.len(), 3);
        assert!(state.current.is_none());
    }

    #[tokio::test]
    async fn delayed_raw_response_does_not_invalidate_renewed_credentials() {
        let mut server = mockito::Server::new_async().await;
        let old = server
            .mock("POST", "/old")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Unknown access token")
            .create_async()
            .await;
        let expired = server
            .mock("GET", "/rotate")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let renewed = server
            .mock("GET", "/rotate")
            .match_header("authorization", token_header("token2"))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let check = server
            .mock("GET", "/check")
            .match_header("authorization", token_header("token2"))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let (client, session) = test_hardware_client(&server.url());
        let delayed_client = client.clone();
        let old_url = client.url("old");
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let delayed = tokio::spawn(async move {
            let response = delayed_client
                .post_pairs_url_raw(&old_url, &[], None)
                .await
                .unwrap();
            ready_tx.send(()).unwrap();
            release_rx.await.unwrap();
            delayed_client.handle_created_location(response).await
        });
        ready_rx.await.unwrap();
        let _: serde_json::Value = client.get("rotate").await.unwrap();
        release_tx.send(()).unwrap();
        assert!(matches!(
            delayed.await.unwrap(),
            Err(LpError::Api { status: 401, .. })
        ));
        let _: serde_json::Value = client.get("check").await.unwrap();
        for mock in [old, expired, renewed, check] {
            mock.assert_async().await;
        }
        let state = session.state.lock().await;
        assert_eq!(state.minted, 2);
        assert_eq!(state.current.as_ref().unwrap().access_token.token, "token2");
        assert_eq!(
            state
                .rejected
                .iter()
                .map(|c| c.access_token.token.as_str())
                .collect::<Vec<_>>(),
            ["token1", "token1"]
        );
    }

    #[tokio::test]
    async fn unsigned_and_unrelated_errors_do_not_invalidate_or_retry() {
        let mut api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let permission = api
            .mock("GET", "/permission")
            .match_header("authorization", token_header("token1"))
            .with_status(401)
            .with_body("Unauthorized: missing permission")
            .create_async()
            .await;
        let non_401 = api
            .mock("GET", "/other")
            .match_header("authorization", token_header("token1"))
            .with_status(403)
            .with_body("Expired token")
            .create_async()
            .await;
        let unsigned = foreign
            .mock("GET", "/foreign")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let (client, session) = test_hardware_client(&api.url());
        let _: Result<serde_json::Value> = client.get("permission").await;
        let _: Result<serde_json::Value> = client.get("other").await;
        let result: Result<serde_json::Value> =
            client.get_url(&format!("{}/foreign", foreign.url())).await;
        assert!(matches!(result, Err(LpError::Api { status: 401, .. })));
        for mock in [permission, non_401, unsigned] {
            mock.assert_async().await;
        }
        let state = session.state.lock().await;
        assert_eq!(state.minted, 1);
        assert!(state.rejected.is_empty());
    }

    #[tokio::test]
    async fn cross_origin_redirects_never_attribute_unsigned_rejections_to_the_token() {
        let mut api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let outward = api
            .mock("GET", "/outward")
            .match_header("authorization", token_header("token1"))
            .with_status(302)
            .with_header("location", &format!("{}/reject", foreign.url()))
            .create_async()
            .await;
        let foreign_rejection = foreign
            .mock("GET", "/reject")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(401)
            .with_body("Expired token")
            .create_async()
            .await;
        let bounce_start = api
            .mock("GET", "/bounce")
            .match_header("authorization", token_header("token1"))
            .with_status(302)
            .with_header("location", &format!("{}/bounce", foreign.url()))
            .create_async()
            .await;
        let bounce_back = foreign
            .mock("GET", "/bounce")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(302)
            .with_header("location", &format!("{}/return", api.url()))
            .create_async()
            .await;
        let not_followed = api.mock("GET", "/return").expect(0).create_async().await;
        let (client, session) = test_hardware_client(&api.url());
        let result: Result<serde_json::Value> = client.get("outward").await;
        assert!(matches!(result, Err(LpError::Api { status: 401, .. })));
        let result: Result<serde_json::Value> = client.get("bounce").await;
        assert!(matches!(result, Err(LpError::Api { status: 302, .. })));
        for mock in [
            outward,
            foreign_rejection,
            bounce_start,
            bounce_back,
            not_followed,
        ] {
            mock.assert_async().await;
        }
        let state = session.state.lock().await;
        assert_eq!(state.minted, 1);
        assert!(state.rejected.is_empty());
    }

    #[tokio::test]
    async fn cleanup_failure_preserves_rejections_across_request_methods() {
        let mut server = mockito::Server::new_async().await;
        let (client, session) = test_hardware_client(&server.url());
        let client = client.with_max_retries(2).with_retry_delay_ms(0);
        session.fail_invalidation.store(true, Ordering::SeqCst);
        let params = HashMap::new();
        let dir = tempfile::tempdir().unwrap();
        for (index, (method, path)) in [
            ("POST", "/post"),
            ("PATCH", "/patch"),
            ("DELETE", "/delete"),
            ("POST", "/location"),
            ("POST", "/multipart"),
            ("POST", "/raw"),
            ("GET", "/download"),
            ("GET", "/progress"),
            ("GET", "/redirect"),
        ]
        .iter()
        .enumerate()
        {
            let token = format!("token{}", index + 1);
            let rejected = server
                .mock(method, *path)
                .match_header("authorization", token_header(&token))
                .with_status(401)
                .with_body("Expired token")
                .create_async()
                .await;
            let url = format!("{}{path}", server.url());
            let result: Result<()> = match *path {
                "/post" => client
                    .post_url::<serde_json::Value>(&url, &params)
                    .await
                    .map(|_| ()),
                "/patch" => client
                    .patch_url::<serde_json::Value>(&url, &params)
                    .await
                    .map(|_| ()),
                "/delete" => client.delete_url_ok(&url).await,
                "/location" => client
                    .post_url_created_location(&url, &params)
                    .await
                    .map(|_| ()),
                "/multipart" => client
                    .post_multipart_created_location(path, reqwest::multipart::Form::new())
                    .await
                    .map(|_| ()),
                "/raw" => {
                    let resp = client.post_pairs_url_raw(&url, &[], None).await.unwrap();
                    client
                        .handle_response::<serde_json::Value>(resp)
                        .await
                        .map(|_| ())
                }
                "/download" => client
                    .download_to_file(&url, &dir.path().join("file"))
                    .await
                    .map(|_| ()),
                "/progress" => client
                    .download_to_file_with_progress(&url, &dir.path().join("file"), |_, _| {})
                    .await
                    .map(|_| ()),
                "/redirect" => client.resolve_redirect(&url).await.map(|_| ()),
                _ => unreachable!(),
            };
            match result {
                Err(LpError::Api {
                    status: 401,
                    message,
                }) => {
                    if matches!(*path, "/download" | "/progress") {
                        assert!(message.contains("Expired token"), "{path}");
                    } else {
                        assert_eq!(message, "Expired token", "{path}");
                    }
                }
                other => panic!("unexpected result for {path}: {other:?}"),
            }
            rejected.assert_async().await;
        }
        let state = session.state.lock().await;
        assert_eq!(state.minted, 9);
        assert_eq!(state.rejected.len(), 9);
        assert!(state.current.is_none());
    }

    /// Verify that an authenticated client stores its credentials.
    #[test]
    fn authenticated_client_stores_credentials() {
        use crate::auth::Credentials;
        let creds = Credentials::new("lpcli", "token", "secret");
        let client = LaunchpadClient::new(Some(creds.clone()));
        assert_eq!(
            client.credentials.as_ref().unwrap().access_token.token,
            "token"
        );
    }
}
