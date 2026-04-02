//! Error types for lpcli.
//!
//! All public API functions return [`LpError`] wrapped in a [`Result`].
//! Use [`thiserror`] derive macros to produce meaningful, structured errors.

use thiserror::Error;

/// Top-level error type for the lpcli library.
#[derive(Debug, Error)]
pub enum LpError {
    /// An unclassified HTTP transport error occurred.
    ///
    /// Use the more specific [`LpError::Timeout`], [`LpError::Connect`], and
    /// [`LpError::Tls`] variants where possible; this variant is the fallback.
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),

    /// The request timed out before a response was received.
    #[error("Request timed out: {0}")]
    Timeout(String),

    /// A TCP connection error prevented the request from reaching the server
    /// (DNS failure, connection refused, etc.).
    #[error("Connection error: {0}")]
    Connect(String),

    /// A TLS/SSL handshake or certificate error occurred.
    #[error("TLS error: {0}")]
    Tls(String),

    /// The Launchpad API rate-limited this request (HTTP 429 Too Many Requests).
    ///
    /// `retry_after_secs` is populated from the `Retry-After` response header
    /// when present.
    #[error("Rate limited by Launchpad. Retry after {retry_after_secs:?} seconds.")]
    RateLimit { retry_after_secs: Option<u64> },

    /// The Launchpad API returned a non-success status code.
    #[error("Launchpad API error {status}: {message}")]
    Api { status: u16, message: String },

    /// A JSON (de)serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An OAuth signing or token error.
    #[error("OAuth error: {0}")]
    OAuth(String),

    /// An I/O error (e.g. reading/writing the credential file).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A URL parse error.
    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),

    /// The user is not authenticated; they must run `lpcli login` first.
    #[error("Not authenticated. Run `lpcli login` to authenticate with Launchpad.")]
    NotAuthenticated,

    /// The requested resource was not found on Launchpad.
    #[error("Not found: {0}")]
    NotFound(String),

    /// A configuration error (e.g. malformed credential file).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Any other error with an arbitrary message.
    #[error("{0}")]
    Other(String),
}

/// Convenience alias used throughout the library.
pub type Result<T> = std::result::Result<T, LpError>;

impl From<reqwest::Error> for LpError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            LpError::Timeout(err.to_string())
        } else if err.is_connect() {
            // TLS/SSL errors surface through the connect path; detect them by
            // inspecting the error message for well-known keywords.
            let msg = err.to_string().to_ascii_lowercase();
            if msg.contains("tls")
                || msg.contains("ssl")
                || msg.contains("certificate")
                || msg.contains("handshake")
            {
                LpError::Tls(err.to_string())
            } else {
                LpError::Connect(err.to_string())
            }
        } else {
            LpError::Http(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_authenticated_error_display() {
        let err = LpError::NotAuthenticated;
        assert!(err.to_string().contains("lpcli login"));
    }

    #[test]
    fn not_found_error_display() {
        let err = LpError::NotFound("~ubuntu".to_string());
        assert_eq!(err.to_string(), "Not found: ~ubuntu");
    }

    #[test]
    fn api_error_display() {
        let err = LpError::Api {
            status: 404,
            message: "Resource not found".to_string(),
        };
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("Resource not found"));
    }

    #[test]
    fn oauth_error_display() {
        let err = LpError::OAuth("invalid token".to_string());
        assert!(err.to_string().contains("invalid token"));
    }

    #[test]
    fn config_error_display() {
        let err = LpError::Config("missing consumer_key".to_string());
        assert!(err.to_string().contains("missing consumer_key"));
    }
}
