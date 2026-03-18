//! Error types for lpcli.
//!
//! All public API functions return [`LpError`] wrapped in a [`Result`].
//! Use [`thiserror`] derive macros to produce meaningful, structured errors.

use thiserror::Error;

/// Top-level error type for the lpcli library.
#[derive(Debug, Error)]
pub enum LpError {
    /// An HTTP transport error occurred when contacting the Launchpad API.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

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
