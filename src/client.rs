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

fn api_http_client(base_url: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .redirect(redirect_policy(Some(base_url)))
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

/// Default maximum `Retry-After` delay honoured by [`LaunchpadClient`] before
/// giving up on automatic retries (5 minutes).
pub const DEFAULT_MAX_RETRY_AFTER: Duration = Duration::from_mins(5);

/// Maximum size in bytes read by [`LaunchpadClient::get_public_text_url()`].
const MAX_PUBLIC_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Total request and body timeout enforced by [`LaunchpadClient::get_public_text_url()`].
const PUBLIC_TEXT_TIMEOUT: Duration = Duration::from_secs(120);

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
    /// Maximum `Retry-After` delay honoured on HTTP `429` responses.
    /// `Some(DEFAULT_MAX_RETRY_AFTER)` by default; `None` disables the cap.
    max_retry_after: Option<Duration>,
}

impl LaunchpadClient {
    /// Create a new client.
    ///
    /// Pass `Some(creds)` for authenticated calls; `None` for anonymous access.
    pub fn new(credentials: Option<Credentials>) -> Self {
        Self {
            http: api_http_client(API_BASE),
            credentials,
            hardware: None,
            base_url: API_BASE.to_string(),
            max_retries: 0,
            initial_retry_delay_ms: 1_000,
            max_retry_after: Some(DEFAULT_MAX_RETRY_AFTER),
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
            http: api_http_client(API_BASE),
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
        self.http = api_http_client(&self.base_url);
        self
    }

    /// Set the maximum number of retry attempts for rate-limited (429) and
    /// server-error (5xx) responses.
    ///
    /// `0` (the default) disables retries. The backoff starts at
    /// `initial_retry_delay_ms` milliseconds, doubling after each attempt, and
    /// is clamped per attempt to [`Self::with_max_retry_after()`]. For 429
    /// responses, a valid `Retry-After` header (delay-seconds or HTTP-date)
    /// overrides the backoff, including a zero delay for past dates.
    ///
    /// Note that `max_retry_after` bounds each retry attempt individually, so
    /// the worst-case total sleep across `max_retries` attempts can be up to
    /// `max_retries * max_retry_after` (for example, up to 10 minutes with
    /// `max_retries = 2` and the default 5-minute [`DEFAULT_MAX_RETRY_AFTER`]),
    /// in addition to per-request timeouts. If a `429` `Retry-After` delay
    /// exceeds `max_retry_after`, retries stop immediately and return
    /// [`LpError::RateLimit`] with the server's `retry_after_secs`.
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

    /// Set the maximum per-attempt retry delay honoured when retrying responses.
    ///
    /// Defaults to `Some(`[`DEFAULT_MAX_RETRY_AFTER`]`)` (5 minutes). When a
    /// `429` response carries a valid `Retry-After` delay larger than this
    /// limit, retries stop immediately without sleeping and [`LpError::RateLimit`]
    /// is returned with the server's advertised `retry_after_secs`.
    /// Exponential backoff sleeps (for `5xx` responses and `429` responses
    /// without a usable `Retry-After`) are clamped per attempt to this limit.
    ///
    /// Pass `None` to remove the limit on both backoff and `Retry-After`;
    /// even with `None`, a `Retry-After` delay that overflows the runtime
    /// timer stops retries immediately and returns [`LpError::RateLimit`]
    /// rather than retrying early.
    pub fn with_max_retry_after(mut self, max_retry_after: Option<Duration>) -> Self {
        self.max_retry_after = max_retry_after;
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

    /// Verify that `url` shares the configured Launchpad API origin.
    pub(crate) fn ensure_same_api_origin(&self, url: &str, context: &str) -> Result<()> {
        if !same_origin(url, &self.base_url) {
            return Err(LpError::Other(format!(
                "Refusing to follow cross-origin Launchpad {context}: {}",
                crate::download::redact_url_for_display(url)
            )));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    /// Perform an authenticated GET request and deserialise the JSON body.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_url(&self.url(path)).await
    }

    /// Like [`Self::get()`], but also keep the exact JSON that Launchpad
    /// returned.
    pub async fn get_raw<T: DeserializeOwned>(&self, path: &str) -> Result<Raw<T>> {
        self.get_url_raw(&self.url(path)).await
    }

    /// Like [`Self::get_url()`], but also keep the exact JSON that Launchpad
    /// returned, for output that must reproduce Launchpad's representation.
    pub async fn get_url_raw<T: DeserializeOwned>(&self, url: &str) -> Result<Raw<T>> {
        Raw::from_value(self.get_url(url).await?)
    }

    /// Read back an entry after a mutation has already succeeded.
    ///
    /// A failed read-back becomes [`LpError::Partial`], so callers can tell
    /// users that Launchpad may already contain the change.
    pub async fn get_created_raw<T: DeserializeOwned>(
        &self,
        location: &str,
        description: &str,
    ) -> Result<Raw<T>> {
        self.get_url_raw(location).await.map_err(|error| {
            LpError::Partial(format!(
                "{description} succeeded, but reading the result from {location} failed: {error}"
            ))
        })
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
    /// empty body and provide the created resource URL in `Location`. The
    /// location is resolved against the response URL and must be on the
    /// configured API origin before it is returned, because callers fetch it
    /// with this credential-bearing client.
    ///
    /// # Errors
    ///
    /// Besides HTTP status errors, returns [`LpError::Partial`] when a
    /// successful response has no valid `Location` header or reports a
    /// location outside the configured API origin. The request may already
    /// have succeeded in either case.
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
        let same_api_origin = same_origin(url, &self.base_url);
        let hardware_credentials = if let Some(session) = &self.hardware {
            if same_api_origin {
                Some(session.access_credentials().await?)
            } else {
                None
            }
        } else {
            None
        };
        let signing_credentials = if self.hardware.is_some() {
            hardware_credentials.as_ref()
        } else if same_api_origin {
            self.credentials.as_ref()
        } else {
            None
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
    /// HTTP-date. Future dates wait until that date unless the delay exceeds
    /// `max_retry_after` (`Some(DEFAULT_MAX_RETRY_AFTER)` by default) or
    /// overflows the runtime timer, in which case retries stop immediately and
    /// the final 429 response is returned so callers receive [`LpError::RateLimit`]
    /// with the server's `retry_after_secs`. Current/past dates do not wait.
    /// Missing, invalid or unrepresentable hints use the current backoff
    /// (which starts at `initial_retry_delay_ms`, doubles after each
    /// unsuccessful attempt, and is clamped per attempt to `max_retry_after`).
    /// 5xx responses continue to use the clamped exponential backoff.
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
                && let Some(sleep_for) = calculate_retry_delay(
                    status,
                    resp.headers(),
                    Duration::from_millis(delay_ms),
                    self.max_retry_after,
                    SystemTime::now(),
                )
            {
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
    ///
    /// Callers fetch the returned location with this client, which signs
    /// requests with any stored credentials. The location is therefore
    /// resolved against the response URL and rejected unless it is on the
    /// configured API origin (exact scheme, host and effective port), so a
    /// foreign `Location` can never receive Launchpad credentials.
    ///
    /// # Errors
    ///
    /// Besides HTTP status errors, returns [`LpError::Partial`] when a
    /// successful response has no valid `Location` header, or when the
    /// location is outside the configured API origin. The mutation may
    /// already have succeeded.
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
                LpError::Partial(
                    "Launchpad response did not include a valid Location header. The \
                     request may already have succeeded; verify the result on Launchpad \
                     before retrying."
                        .to_string(),
                )
            })?;

        self.trusted_created_location(resp.url(), location)
    }

    /// Resolve a created-resource `location` against the URL of the response
    /// that carried it, and require the result to be on the configured API
    /// origin before any caller fetches it with credentials.
    fn trusted_created_location(&self, response_url: &url::Url, location: &str) -> Result<String> {
        let resolved = response_url
            .join(location)
            .ok()
            .filter(|resolved| same_origin(resolved.as_str(), &self.base_url));
        resolved.map(String::from).ok_or_else(|| {
            let api_origin = url::Url::parse(&self.base_url)
                .map(|base| base.origin().ascii_serialization())
                .unwrap_or_else(|_| self.base_url.clone());
            LpError::Partial(format!(
                "Launchpad reported the created resource at '{location}', which is not \
                 on the configured API origin '{api_origin}'. It was not fetched, \
                 because requests from this client carry Launchpad credentials. The \
                 request may already have succeeded; verify the result on Launchpad \
                 before retrying."
            ))
        })
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
        self.download_to_file_with_progress(url, dest, |_, _| {})
            .await
    }

    /// Download a file from `url` into `dest`, reporting progress via a callback.
    ///
    /// The callback `on_progress` is invoked with `(bytes_downloaded_so_far, total_size)`.
    /// `total_size` is `None` if the server did not provide a `Content-Length` header.
    ///
    /// Use [`crate::download::MilestoneProgress`] to report at fixed byte milestones.
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
        let download_client = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(300))
            .redirect(redirect_policy(Some(&self.base_url)))
            .build()
            .map_err(|e| LpError::Other(format!("Failed to build download client: {e}")))?;

        let req = download_client.get(url);

        let (req, credentials) = self.authorize_request(req, url).await?;

        let safe_url = crate::download::redact_url_for_display(url);
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                LpError::Timeout(format!("Connection timed out downloading {safe_url}: {e}"))
            } else if e.is_connect() {
                LpError::Connect(format!("Connection failed downloading {safe_url}: {e}"))
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
                message: format!("Failed to download {safe_url}: HTTP {code} — {body}"),
            });
        }

        stream_response_to_file(resp, &safe_url, dest, false, on_progress).await
    }

    /// Validate that `url` is a trusted Launchpad or Librarian URL suitable for
    /// unauthenticated public text or log downloads.
    pub(crate) fn validate_public_url(&self, url: &str) -> Result<url::Url> {
        let parsed = url::Url::parse(url)?;
        if !is_trusted_public_text_url(&parsed, &self.base_url) {
            return Err(LpError::Other(format!(
                "Refusing to access untrusted public URL: {}",
                crate::download::redact_url_for_display(url)
            )));
        }
        Ok(parsed)
    }

    /// Download a public file from a Launchpad or Librarian URL into `dest`
    /// without attaching OAuth or hardware-bound credentials.
    ///
    /// Both the initial URL and any redirect hops are restricted to HTTPS URLs
    /// under `launchpad.net` or `launchpadlibrarian.net`, or the client's
    /// configured `base_url` origin (for local test servers). Creates `dest`
    /// exclusively (`create_new(true)`) so existing files and symlinks are not
    /// overwritten or followed, and removes any partial file it created if the
    /// transfer fails. Invokes `on_progress` with
    /// `(bytes_downloaded_so_far, total_size)`.
    ///
    /// Use [`crate::download::MilestoneProgress`] to report at fixed byte milestones.
    pub async fn download_public_to_file_with_progress<F>(
        &self,
        url: &str,
        dest: &std::path::Path,
        on_progress: F,
    ) -> Result<u64>
    where
        F: FnMut(u64, Option<u64>),
    {
        let parsed = self.validate_public_url(url)?;
        let safe_url = crate::download::redact_url_for_display(url);
        let download_client = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(300))
            .redirect(public_trusted_redirect_policy(&self.base_url))
            .build()
            .map_err(|e| LpError::Other(format!("Failed to build download client: {e}")))?;

        let req = download_client.get(parsed);
        let resp = self.send_with_retry(req).await.map_err(|e| {
            if e.is_timeout() {
                LpError::Timeout(format!("Connection timed out downloading {safe_url}: {e}"))
            } else if e.is_connect() {
                LpError::Connect(format!("Connection failed downloading {safe_url}: {e}"))
            } else {
                LpError::Http(e)
            }
        })?;

        let status = resp.status();
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
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "(could not read body)".to_string());
            return Err(LpError::Api {
                status: code,
                message: format!("Failed to download {safe_url}: HTTP {code} — {body}"),
            });
        }

        stream_response_to_file(resp, &safe_url, dest, true, on_progress).await
    }

    /// Perform an authenticated GET against an absolute URL and return the
    /// JSON response body as a `Vec<String>`.
    ///
    /// Use this for Launchpad custom GET methods that return a JSON list of
    /// strings (e.g. `sourceFileUrls`, `binaryFileUrls`).
    pub async fn get_url_string_list(&self, url: &str) -> Result<Vec<String>> {
        self.get_url(url).await
    }

    /// Fetch public text content from a Launchpad or Librarian URL without
    /// attaching OAuth or hardware-bound credentials.
    ///
    /// Both the initial URL and any redirect hops are restricted to HTTPS URLs
    /// under `launchpad.net` or `launchpadlibrarian.net`, or the client's
    /// configured `base_url` origin (for local test servers). Response bodies
    /// are capped at 16 MiB and decoded lossily if non-UTF-8 bytes appear.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Url`] on malformed URLs, [`LpError::NotFound`] on HTTP
    /// 404, [`LpError::RateLimit`] on HTTP 429, [`LpError::Timeout`] on request or
    /// stream timeout, [`LpError::Api`] on other non-success status codes, or
    /// [`LpError::Other`] when the URL/redirect target is untrusted or the
    /// response exceeds the size limit.
    pub async fn get_public_text_url(&self, url: &str) -> Result<String> {
        self.get_public_text_url_with_limit(url, MAX_PUBLIC_TEXT_BYTES)
            .await
    }

    async fn get_public_text_url_with_limit(&self, url: &str, max_bytes: usize) -> Result<String> {
        self.get_public_text_url_with_options(url, max_bytes, PUBLIC_TEXT_TIMEOUT)
            .await
    }

    async fn get_public_text_url_with_options(
        &self,
        url: &str,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<String> {
        let parsed = url::Url::parse(url)?;
        if !is_trusted_public_text_url(&parsed, &self.base_url) {
            return Err(LpError::Other(format!(
                "Refusing to fetch untrusted public text URL: {}",
                crate::download::redact_url_for_display(url)
            )));
        }
        let http = public_text_http_client(&self.base_url, timeout)?;
        let req = http.get(parsed);
        let resp = self.send_with_retry(req).await?;
        let status = resp.status();
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
        if resp
            .content_length()
            .is_some_and(|len| len > max_bytes as u64)
        {
            return Err(LpError::Other(format!(
                "Public text response exceeds maximum size ({max_bytes} bytes)."
            )));
        }

        let buf = tokio::time::timeout(timeout, async {
            let mut buf = Vec::new();
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if buf.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(LpError::Other(format!(
                        "Public text response exceeds maximum size ({max_bytes} bytes)."
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(buf)
        })
        .await
        .map_err(|_| {
            LpError::Timeout(format!(
                "Timed out reading public text from {}",
                crate::download::redact_url_for_display(url)
            ))
        })??;

        Ok(match String::from_utf8(buf) {
            Ok(text) => text,
            Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
        })
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

/// Calculate the duration to wait before the next retry attempt, or `None` when
/// automatic retries should stop immediately.
///
/// For `429 Too Many Requests` responses, a valid `Retry-After` header delay
/// takes precedence over `backoff` when it does not exceed `max_retry_after`
/// and can be scheduled by the Tokio timer. If the server's `Retry-After`
/// delay exceeds `max_retry_after` (or overflows the timer), returns `None`
/// so [`LaunchpadClient::send_with_retry`] fails fast instead of sleeping
/// longer than the cap or retrying earlier than the server requested.
/// All other retryable statuses (such as `5xx` server errors), or responses
/// without a usable `Retry-After` header, fall back to `backoff` clamped to
/// `max_retry_after` (or `None` if that overflows the timer).
fn calculate_retry_delay(
    status: StatusCode,
    headers: &HeaderMap,
    backoff: Duration,
    max_retry_after: Option<Duration>,
    now: SystemTime,
) -> Option<Duration> {
    let capped_backoff = || {
        let clamped = max_retry_after.map_or(backoff, |max| backoff.min(max));
        tokio::time::Instant::now()
            .checked_add(clamped)
            .map(|_| clamped)
    };
    if status == StatusCode::TOO_MANY_REQUESTS {
        match parse_retry_after_header(headers, now) {
            Some(delay) => {
                if max_retry_after.is_some_and(|max| delay > max) {
                    return None;
                }
                tokio::time::Instant::now()
                    .checked_add(delay)
                    .map(|_| delay)
            }
            None => capped_backoff(),
        }
    } else {
        capped_backoff()
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

fn is_trusted_public_text_url(url: &url::Url, base_url: &str) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if same_origin(url.as_str(), base_url) {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.');
    ["launchpad.net", "launchpadlibrarian.net"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn public_trusted_redirect_policy(base_url: &str) -> reqwest::redirect::Policy {
    let trusted_base = base_url.to_string();
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        if !is_trusted_public_text_url(attempt.url(), &trusted_base) {
            return attempt.error("refusing redirect to untrusted host");
        }
        attempt.follow()
    })
}

fn public_text_http_client(base_url: &str, timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .timeout(timeout)
        .redirect(public_trusted_redirect_policy(base_url))
        .build()
        .map_err(|e| LpError::Other(format!("Failed to build HTTP client: {e}")))
}

async fn stream_response_to_file<F>(
    resp: reqwest::Response,
    safe_url: &str,
    dest: &std::path::Path,
    create_new: bool,
    mut on_progress: F,
) -> Result<u64>
where
    F: FnMut(u64, Option<u64>),
{
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let content_length = resp.content_length();
    let mut file = if create_new {
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .await
    } else {
        tokio::fs::File::create(dest).await
    }
    .map_err(|e| {
        LpError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to create file '{}': {e}", dest.display()),
        ))
    })?;

    let result: Result<u64> = async {
        let mut total_bytes: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                LpError::Other(format!(
                    "Network error while downloading '{}': {e}",
                    dest.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| safe_url.to_string()),
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
    .await;

    if result.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(dest).await;
    }

    result
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

/// A typed Launchpad value together with the exact JSON it was decoded from.
///
/// The typed `value` keeps only the fields lpcli models, and re-encoding it
/// could change Launchpad's representation. `raw` is the unmodified JSON
/// (field names, `null`s, links and values), for output that must match
/// Launchpad's own representation.
#[derive(Debug, Clone, PartialEq)]
pub struct Raw<T> {
    /// The decoded value.
    pub value: T,
    /// The JSON exactly as Launchpad returned it.
    pub raw: serde_json::Value,
}

impl<T: DeserializeOwned> Raw<T> {
    /// Decode `raw` into `T`, keeping the original JSON alongside it.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Json`] if `raw` does not match `T`.
    pub fn from_value(raw: serde_json::Value) -> Result<Self> {
        let value = T::deserialize(&raw)?;
        Ok(Self { value, raw })
    }
}

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
                Some(next) => {
                    client.ensure_same_api_origin(&next, "next_collection_link")?;
                    url = next;
                }
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

    /// Fetch every page and return Launchpad's collection shape with exact
    /// entries.
    ///
    /// The first page's `total_size` or `total_size_link` is passed through.
    /// `next_collection_link` is omitted because every page was fetched.
    pub async fn fetch_all_raw(client: &LaunchpadClient, first_url: &str) -> Result<Raw<Vec<T>>> {
        let first: Raw<Collection<serde_json::Value>> = client.get_url_raw(first_url).await?;
        let mut raw_entries = first.value.entries;
        let mut next = first.value.next_collection_link;
        while let Some(url) = next {
            client.ensure_same_api_origin(&url, "next_collection_link")?;
            let page: Collection<serde_json::Value> = client.get_url(&url).await?;
            raw_entries.extend(page.entries);
            next = page.next_collection_link;
        }
        let value = raw_entries
            .iter()
            .map(T::deserialize)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut raw = serde_json::Map::new();
        for key in ["total_size", "total_size_link"] {
            if let Some(field) = first.raw.get(key) {
                raw.insert(key.into(), field.clone());
            }
        }
        raw.insert("start".into(), 0.into());
        raw.insert("entries".into(), raw_entries.into());
        Ok(Raw {
            value,
            raw: raw.into(),
        })
    }

    /// Like [`Self::fetch_page()`], but return the whole page, together with
    /// the exact JSON Launchpad returned for it (including `total_size`,
    /// `total_size_link`, `start` and `next_collection_link`, when present).
    pub async fn fetch_page_raw(client: &LaunchpadClient, url: &str) -> Result<Raw<Self>> {
        client.get_url_raw(url).await
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
        client.http = api_http_client(base);
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
    fn default_max_retry_after_is_five_minutes_and_configurable() {
        assert_eq!(DEFAULT_MAX_RETRY_AFTER, Duration::from_secs(300));
        let default_client = LaunchpadClient::new(None);
        assert_eq!(
            default_client.max_retry_after,
            Some(DEFAULT_MAX_RETRY_AFTER)
        );
        let custom_client = default_client
            .clone()
            .with_max_retry_after(Some(Duration::from_secs(30)));
        assert_eq!(custom_client.max_retry_after, Some(Duration::from_secs(30)));
        let unbounded_client = custom_client.with_max_retry_after(None);
        assert_eq!(unbounded_client.max_retry_after, None);
    }

    #[test]
    fn calculate_retry_delay_uses_backoff_only_without_usable_429_advice() {
        let backoff = Duration::from_millis(100);
        let default_cap = Some(DEFAULT_MAX_RETRY_AFTER);
        for headers in [
            HeaderMap::new(),
            retry_headers("invalid"),
            retry_headers("18446744073709551615"),
        ] {
            assert_eq!(
                calculate_retry_delay(
                    StatusCode::TOO_MANY_REQUESTS,
                    &headers,
                    backoff,
                    default_cap,
                    date(),
                ),
                Some(backoff)
            );
            assert_eq!(
                calculate_retry_delay(
                    StatusCode::TOO_MANY_REQUESTS,
                    &headers,
                    backoff,
                    Some(Duration::ZERO),
                    date(),
                ),
                Some(Duration::ZERO)
            );
        }
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(HTTP_DATE),
                backoff,
                default_cap,
                date(),
            ),
            Some(Duration::ZERO)
        );
        // HTTP-date parsing does not broaden which statuses honor Retry-After.
        assert_eq!(
            calculate_retry_delay(
                StatusCode::SERVICE_UNAVAILABLE,
                &retry_headers("0"),
                backoff,
                default_cap,
                date(),
            ),
            Some(backoff)
        );
    }

    #[test]
    fn calculate_retry_delay_clamps_exponential_backoff_to_max_retry_after() {
        let large_backoff = Duration::from_secs(600);
        assert_eq!(
            calculate_retry_delay(
                StatusCode::INTERNAL_SERVER_ERROR,
                &HeaderMap::new(),
                large_backoff,
                Some(DEFAULT_MAX_RETRY_AFTER),
                date(),
            ),
            Some(DEFAULT_MAX_RETRY_AFTER)
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("invalid"),
                Duration::from_millis(100),
                Some(Duration::from_millis(50)),
                date(),
            ),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::SERVICE_UNAVAILABLE,
                &HeaderMap::new(),
                large_backoff,
                None,
                date(),
            ),
            Some(large_backoff)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn calculate_retry_delay_fails_fast_when_unbounded_retry_after_overflows_timer() {
        tokio::time::advance(Duration::from_secs(1_000)).await;
        let overflow_secs = (i64::MAX as u64) - 500;
        let delay = Duration::from_secs(overflow_secs);
        assert!(
            parse_retry_after(&overflow_secs.to_string(), SystemTime::UNIX_EPOCH).is_some(),
            "delay should remain representable against UNIX_EPOCH"
        );
        assert!(
            tokio::time::Instant::now().checked_add(delay).is_none(),
            "delay should overflow tokio::time::Instant after advancing clock"
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(&overflow_secs.to_string()),
                Duration::from_millis(100),
                None,
                SystemTime::UNIX_EPOCH,
            ),
            None
        );
    }

    #[test]
    fn calculate_retry_delay_enforces_max_retry_after_bound() {
        let backoff = Duration::from_millis(100);
        let cap = Some(DEFAULT_MAX_RETRY_AFTER);

        // Within and exactly at the 5-minute bound (integer seconds & HTTP-date).
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("300"),
                backoff,
                cap,
                date(),
            ),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(HTTP_DATE),
                backoff,
                cap,
                date() - Duration::from_secs(300),
            ),
            Some(Duration::from_secs(300))
        );

        // Exceeding the cap (even by 1ns or 1s) stops retries immediately.
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("301"),
                backoff,
                cap,
                date(),
            ),
            None
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(HTTP_DATE),
                backoff,
                cap,
                date() - Duration::from_secs(300) - Duration::from_nanos(1),
            ),
            None
        );

        // A zero-second cap allows 0s/past Retry-After and rejects positive delays.
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("0"),
                backoff,
                Some(Duration::ZERO),
                date(),
            ),
            Some(Duration::ZERO)
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("1"),
                backoff,
                Some(Duration::ZERO),
                date(),
            ),
            None
        );

        // Disabling the cap allows long delays (integer seconds & HTTP-date).
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers("3600"),
                backoff,
                None,
                date(),
            ),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(HTTP_DATE),
                backoff,
                None,
                date() - Duration::from_secs(3600),
            ),
            Some(Duration::from_secs(3600))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_sleep_preserves_server_delay() {
        // Exercise the same delay selection and timer used by send_with_retry,
        // without real socket I/O or a wall clock that moves during the test.
        for (header, max_retry_after, delay) in [
            ("2", Some(DEFAULT_MAX_RETRY_AFTER), Duration::from_secs(2)),
            (
                HTTP_DATE,
                Some(DEFAULT_MAX_RETRY_AFTER),
                Duration::from_millis(1250),
            ),
            ("3600", None, Duration::from_secs(3600)),
        ] {
            let selected = calculate_retry_delay(
                StatusCode::TOO_MANY_REQUESTS,
                &retry_headers(header),
                Duration::from_millis(1),
                max_retry_after,
                date() - Duration::from_millis(1250),
            )
            .expect("delay within bound should be selected");
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

    #[tokio::test]
    async fn successful_create_without_location_is_partial() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/bugs")
            .with_status(201)
            .create_async()
            .await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let error = client
            .post_created_location("/bugs", &HashMap::from([("ws.op", "createBug")]))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::Partial);
        assert_eq!(error.exit_code(), 7);
        assert!(error.to_string().contains("may already have succeeded"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn successful_create_with_malformed_location_is_partial() {
        let mut server = mockito::Server::new_async().await;
        let post = server
            .mock("POST", "/bugs")
            .with_status(201)
            .with_header("location", "http://[invalid")
            .create_async()
            .await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let error = client
            .post_created_location("/bugs", &HashMap::from([("ws.op", "createBug")]))
            .await
            .unwrap_err();
        assert_eq!(error.exit_code(), 7);
        assert!(error.to_string().contains("configured API origin"));
        post.assert_async().await;
    }

    #[tokio::test]
    async fn successful_create_with_foreign_location_is_partial_without_fetching_it() {
        let mut server = mockito::Server::new_async().await;
        let post = server
            .mock("POST", "/bugs")
            .with_status(201)
            .with_header("location", "https://evil.example/resource")
            .create_async()
            .await;
        let foreign = server
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let error = client
            .post_created_location("/bugs", &HashMap::from([("ws.op", "createBug")]))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::Partial);
        assert_eq!(error.exit_code(), 7);
        assert!(
            error
                .to_string()
                .contains("not on the configured API origin")
        );
        post.assert_async().await;
        foreign.assert_async().await;
    }

    #[test]
    fn created_locations_are_resolved_and_kept_on_the_api_origin() {
        let client = LaunchpadClient::new(None).with_base_url("https://api.launchpad.net/devel");
        let response = url::Url::parse("https://api.launchpad.net/devel/bugs/42").unwrap();
        for (location, expected) in [
            (
                "https://api.launchpad.net/devel/bugs/42/+watch/7",
                "https://api.launchpad.net/devel/bugs/42/+watch/7",
            ),
            (
                "https://api.launchpad.net:443/devel/bugs/42/+watch/7",
                "https://api.launchpad.net/devel/bugs/42/+watch/7",
            ),
            (
                "/devel/bugs/42/+watch/7",
                "https://api.launchpad.net/devel/bugs/42/+watch/7",
            ),
            (
                "42/+watch/7",
                "https://api.launchpad.net/devel/bugs/42/+watch/7",
            ),
        ] {
            assert_eq!(
                client
                    .trusted_created_location(&response, location)
                    .unwrap(),
                expected,
                "location '{location}'"
            );
        }
        for foreign in [
            "https://evil.example/bugs/42/+watch/7",
            "https://api.launchpad.net.evil.example/devel/bugs/42/+watch/7",
            "http://api.launchpad.net/devel/bugs/42/+watch/7",
            "https://api.launchpad.net:8443/devel/bugs/42/+watch/7",
            "//evil.example/bugs/42/+watch/7",
            "ftp://api.launchpad.net/devel/bugs/42",
            "https://[::1",
        ] {
            match client.trusted_created_location(&response, foreign) {
                Err(LpError::Partial(message)) => assert!(
                    message.contains("not on the configured API origin")
                        && message.contains("may already have succeeded"),
                    "unexpected message for '{foreign}': {message}"
                ),
                other => panic!("'{foreign}' must be rejected, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn foreign_created_location_is_never_signed_or_fetched() {
        use crate::auth::Credentials;
        let mut api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let create = api
            .mock("POST", "/bugs/42")
            .with_status(201)
            .with_header("location", &format!("{}/bugs/42/+watch/7", foreign.url()))
            .expect(1)
            .create_async()
            .await;
        let foreign_request = foreign
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let client = LaunchpadClient::new(Some(Credentials::new("lpcli", "token", "secret")))
            .with_base_url(api.url());
        let params = HashMap::from([("ws.op", "addWatch")]);
        let error = client
            .post_created_location("/bugs/42", &params)
            .await
            .expect_err("a foreign Location must be rejected");
        assert!(
            matches!(&error, LpError::Partial(message) if message.contains("not on the configured API origin")),
            "unexpected error: {error:?}"
        );
        assert!(!error.to_string().contains("secret"));
        create.assert_async().await;
        foreign_request.assert_async().await;
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

    #[test]
    fn is_trusted_public_text_url_allows_launchpad_domains_and_test_origin() {
        let base = "http://127.0.0.1:4567";
        for ok in [
            "https://launchpad.net/ubuntu/+archive/primary/+sourcepub/1/+files/changelog",
            "https://code.launchpad.net/foo",
            "https://launchpadlibrarian.net/12345/changelog",
            "https://foo.launchpadlibrarian.net/12345/changelog",
            "http://127.0.0.1:4567/changelog",
        ] {
            let parsed = url::Url::parse(ok).unwrap();
            assert!(
                is_trusted_public_text_url(&parsed, base),
                "expected trusted URL: {ok}"
            );
        }
        for bad in [
            "http://launchpad.net/changelog",
            "http://launchpadlibrarian.net/12345/changelog",
            "https://evil-launchpad.net/changelog",
            "https://example.invalid/changelog",
            "https://user:pass@launchpad.net/changelog",
            "http://127.0.0.1:9999/changelog",
        ] {
            let parsed = url::Url::parse(bad).unwrap();
            assert!(
                !is_trusted_public_text_url(&parsed, base),
                "expected untrusted URL: {bad}"
            );
        }
    }

    #[tokio::test]
    async fn get_public_text_url_never_sends_credentials_and_validates_redirects() {
        use crate::auth::Credentials;

        let mut server = mockito::Server::new_async().await;
        let mut untrusted = mockito::Server::new_async().await;

        let target_mock = server
            .mock("GET", "/librarian/changelog")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body("unbound (1.0) noble; urgency=low\n\n  * Note.\n\n -- Dev <d@example>  Mon, 01 Jan 2024 00:00:00 +0000\n")
            .create_async()
            .await;
        let redirect_mock = server
            .mock("GET", "/pub/changelog")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(302)
            .with_header("location", &format!("{}/librarian/changelog", server.url()))
            .create_async()
            .await;
        let bad_redirect_mock = server
            .mock("GET", "/pub/evil")
            .with_status(302)
            .with_header("location", &format!("{}/steal", untrusted.url()))
            .create_async()
            .await;
        let untrusted_mock = untrusted
            .mock("GET", "/steal")
            .expect(0)
            .create_async()
            .await;
        let binary_mock = server
            .mock("GET", "/pub/latin1")
            .with_status(200)
            .with_body(vec![b'h', b'i', b' ', 0xff, b'\n'])
            .create_async()
            .await;
        let big_mock = server
            .mock("GET", "/pub/big")
            .with_status(200)
            .with_body("0123456789abcdef")
            .create_async()
            .await;
        let missing_mock = server
            .mock("GET", "/pub/missing")
            .with_status(404)
            .create_async()
            .await;

        let creds = Credentials::new("lpcli", "tok", "sec");
        let client = LaunchpadClient::new(Some(creds)).with_base_url(server.url());

        let text = client
            .get_public_text_url(&format!("{}/pub/changelog", server.url()))
            .await
            .unwrap();
        assert!(text.contains("unbound (1.0)"), "{text}");
        redirect_mock.assert_async().await;
        target_mock.assert_async().await;

        // Untrusted initial URL and untrusted redirect are rejected.
        assert!(
            client
                .get_public_text_url("https://example.invalid/changelog")
                .await
                .is_err()
        );
        assert!(
            client
                .get_public_text_url(&format!("{}/pub/evil", server.url()))
                .await
                .is_err()
        );
        bad_redirect_mock.assert_async().await;
        untrusted_mock.assert_async().await;

        // Lossy UTF-8 decoding succeeds.
        let lossy = client
            .get_public_text_url(&format!("{}/pub/latin1", server.url()))
            .await
            .unwrap();
        assert_eq!(lossy, "hi \u{FFFD}\n");
        binary_mock.assert_async().await;

        // Exceeding byte limit fails.
        assert!(
            client
                .get_public_text_url_with_limit(&format!("{}/pub/big", server.url()), 8)
                .await
                .is_err()
        );
        big_mock.assert_async().await;

        // 404 maps to LpError::NotFound.
        let not_found = client
            .get_public_text_url(&format!("{}/pub/missing", server.url()))
            .await
            .unwrap_err();
        assert!(matches!(not_found, LpError::NotFound(_)), "{not_found:?}");
        missing_mock.assert_async().await;
    }

    #[tokio::test]
    async fn classic_oauth_credentials_are_not_sent_to_cross_origin_urls() {
        let api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let foreign_mock = foreign
            .mock("GET", "/secret-check")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;

        let client = LaunchpadClient::new(Some(Credentials::new(
            "lpcli",
            "classic-token",
            "classic-secret",
        )))
        .with_base_url(api.url());

        let value: serde_json::Value = client
            .get_url(&format!("{}/secret-check", foreign.url()))
            .await
            .unwrap();
        assert_eq!(value["ok"], true);
        foreign_mock.assert_async().await;
    }

    #[tokio::test]
    async fn collection_fetch_all_and_fetch_all_raw_reject_cross_origin_next_link() {
        let mut api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let foreign_mock = foreign
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;

        let body = serde_json::json!({
            "start": 0,
            "total_size": 2,
            "entries": [{"name": "item-1"}],
            "next_collection_link": format!("{}/steal?oauth_token=leak", foreign.url()),
        })
        .to_string();
        let page1_mock = api
            .mock("GET", "/items")
            .with_status(200)
            .with_body(body)
            .expect(2)
            .create_async()
            .await;

        let client = LaunchpadClient::new(Some(Credentials::new("lpcli", "tok", "supersecret")))
            .with_base_url(api.url());

        let first_url = client.url("/items");
        let err = Collection::<serde_json::Value>::fetch_all(&client, &first_url)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cross-origin"), "{err}");
        assert!(!err.contains("oauth_token=leak"), "{err}");
        assert!(!err.contains("supersecret"), "{err}");

        let raw_err = Collection::<serde_json::Value>::fetch_all_raw(&client, &first_url)
            .await
            .unwrap_err()
            .to_string();
        assert!(raw_err.contains("cross-origin"), "{raw_err}");
        assert!(!raw_err.contains("oauth_token=leak"), "{raw_err}");
        assert!(!raw_err.contains("supersecret"), "{raw_err}");

        page1_mock.assert_async().await;
        foreign_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_public_text_url_redacts_query_fragment_and_userinfo_in_error() {
        let client = LaunchpadClient::new(None);
        let err = client
            .get_public_text_url(
                "https://user:pwsecret@example.invalid/pub/changelog?token=toksecret#frag",
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Refusing to fetch untrusted public text URL"),
            "{err}"
        );
        assert!(!err.contains("pwsecret"), "{err}");
        assert!(!err.contains("toksecret"), "{err}");
        assert!(!err.contains("frag"), "{err}");
    }

    #[tokio::test]
    async fn get_public_text_url_times_out_on_stalled_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhello")
                .await;
            let _ = socket.flush().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = LaunchpadClient::new(None).with_base_url(format!("http://{addr}"));
        let err = client
            .get_public_text_url_with_options(
                &format!("http://{addr}/pub/changelog?token=s3cr3t"),
                1024,
                Duration::from_millis(50),
            )
            .await
            .unwrap_err();
        match &err {
            LpError::Timeout(msg) => assert!(!msg.contains("s3cr3t"), "{msg}"),
            other => panic!("expected LpError::Timeout, got {other:?}"),
        }
        server_task.abort();
    }

    #[tokio::test]
    async fn download_public_to_file_with_progress_enforces_trust_redaction_and_no_creds() {
        let mut server = mockito::Server::new_async().await;
        let mut untrusted = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();

        let target_mock = server
            .mock("GET", "/librarian/buildlog.txt.gz")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(b"gz-bytes")
            .create_async()
            .await;
        let redirect_mock = server
            .mock("GET", "/pub/buildlog.txt.gz")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(302)
            .with_header(
                "location",
                &format!("{}/librarian/buildlog.txt.gz", server.url()),
            )
            .create_async()
            .await;
        let bad_redirect_mock = server
            .mock("GET", "/pub/evil.log")
            .with_status(302)
            .with_header("location", &format!("{}/steal.log", untrusted.url()))
            .create_async()
            .await;
        let untrusted_mock = untrusted
            .mock("GET", "/steal.log")
            .expect(0)
            .create_async()
            .await;
        let missing_mock = server
            .mock("GET", "/pub/missing.log")
            .with_status(404)
            .create_async()
            .await;

        let creds = Credentials::new("lpcli", "toksecret", "secsecret");
        let client = LaunchpadClient::new(Some(creds)).with_base_url(server.url());

        let ok_dest = dir.path().join("ok.txt.gz");
        let mut progress_seen = Vec::new();
        let bytes = client
            .download_public_to_file_with_progress(
                &format!("{}/pub/buildlog.txt.gz", server.url()),
                &ok_dest,
                |downloaded, _total| progress_seen.push(downloaded),
            )
            .await
            .unwrap();
        assert_eq!(bytes, 8);
        assert_eq!(std::fs::read(&ok_dest).unwrap(), b"gz-bytes");
        assert_eq!(progress_seen.last().copied(), Some(8));

        let untrusted_err = client
            .download_public_to_file_with_progress(
                "https://user:pwsecret@example.invalid/build.log?oauth_token=tok2#frag",
                &dir.path().join("bad1.log"),
                |_, _| {},
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(untrusted_err.contains("untrusted"), "{untrusted_err}");
        assert!(!untrusted_err.contains("pwsecret"), "{untrusted_err}");
        assert!(!untrusted_err.contains("tok2"), "{untrusted_err}");
        assert!(!untrusted_err.contains("frag"), "{untrusted_err}");

        assert!(
            client
                .download_public_to_file_with_progress(
                    &format!("{}/pub/evil.log", server.url()),
                    &dir.path().join("bad2.log"),
                    |_, _| {},
                )
                .await
                .is_err()
        );

        let not_found = client
            .download_public_to_file_with_progress(
                &format!("{}/pub/missing.log", server.url()),
                &dir.path().join("missing.log"),
                |_, _| {},
            )
            .await
            .unwrap_err();
        assert!(matches!(not_found, LpError::NotFound(_)), "{not_found:?}");

        redirect_mock.assert_async().await;
        target_mock.assert_async().await;
        bad_redirect_mock.assert_async().await;
        untrusted_mock.assert_async().await;
        missing_mock.assert_async().await;
    }

    #[tokio::test]
    async fn download_public_to_file_refuses_overwrite_symlinks_and_cleans_up_partial_writes() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();

        let ok_mock = server
            .mock("GET", "/pub/buildlog.txt.gz")
            .with_status(200)
            .with_body(b"fresh-log")
            .expect(if cfg!(unix) { 3 } else { 1 })
            .create_async()
            .await;
        let not_found_mock = server
            .mock("GET", "/pub/missing.txt.gz")
            .with_status(404)
            .expect(1)
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(server.url());

        // Pre-existing file is not overwritten or removed on AlreadyExists or 404.
        let existing = dir.path().join("existing.txt.gz");
        std::fs::write(&existing, b"keep-me").unwrap();

        let err_exists = client
            .download_public_to_file_with_progress(
                &format!("{}/pub/buildlog.txt.gz", server.url()),
                &existing,
                |_, _| {},
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err_exists, LpError::Io(io_err) if io_err.kind() == std::io::ErrorKind::AlreadyExists),
            "{err_exists:?}"
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep-me");

        let err_404 = client
            .download_public_to_file_with_progress(
                &format!("{}/pub/missing.txt.gz", server.url()),
                &existing,
                |_, _| {},
            )
            .await
            .unwrap_err();
        assert!(matches!(err_404, LpError::NotFound(_)), "{err_404:?}");
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep-me");

        // Symlinks (pointing to a file or dangling) are rejected without modifying target.
        #[cfg(unix)]
        {
            let target = dir.path().join("target.txt");
            std::fs::write(&target, b"untouched").unwrap();
            let symlink = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&target, &symlink).unwrap();
            let err_link = client
                .download_public_to_file_with_progress(
                    &format!("{}/pub/buildlog.txt.gz", server.url()),
                    &symlink,
                    |_, _| {},
                )
                .await
                .unwrap_err();
            assert!(
                matches!(&err_link, LpError::Io(io_err) if io_err.kind() == std::io::ErrorKind::AlreadyExists),
                "{err_link:?}"
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"untouched");

            let dangling = dir.path().join("dangling.txt");
            std::os::unix::fs::symlink(dir.path().join("missing-target"), &dangling).unwrap();
            let err_dangling = client
                .download_public_to_file_with_progress(
                    &format!("{}/pub/buildlog.txt.gz", server.url()),
                    &dangling,
                    |_, _| {},
                )
                .await
                .unwrap_err();
            assert!(
                matches!(&err_dangling, LpError::Io(io_err) if io_err.kind() == std::io::ErrorKind::AlreadyExists),
                "{err_dangling:?}"
            );
            assert!(dangling.symlink_metadata().is_ok());
        }

        // Mid-stream EOF removes the partial file created by this download.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
                let _ = stream.shutdown().await;
            }
        });

        let trunc_base = format!("http://{addr}");
        let trunc_client = LaunchpadClient::new(None).with_base_url(trunc_base.clone());
        let partial_dest = dir.path().join("truncated.txt.gz");
        let trunc_err = trunc_client
            .download_public_to_file_with_progress(
                &format!("{trunc_base}/truncated.txt.gz"),
                &partial_dest,
                |_, _| {},
            )
            .await
            .unwrap_err();
        accept_task.await.unwrap();
        assert!(
            trunc_err.to_string().contains("Network error"),
            "{trunc_err:?}"
        );
        assert!(
            !partial_dest.exists(),
            "partial file must be removed after stream error"
        );

        ok_mock.assert_async().await;
        not_found_mock.assert_async().await;
    }
}
