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
    /// `retry_after_secs` is populated from a valid `Retry-After` response
    /// header, either delay-seconds or an HTTP-date. Dates become seconds
    /// remaining until that date, rounded up; expired dates become zero.
    /// Missing, invalid or unrepresentable values produce `None`.
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

    /// An operating-system keyring error occurred.
    #[error("Keyring error: {0}")]
    Keyring(String),

    /// An I/O error (e.g. reading/writing the credential file).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A URL parse error.
    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),

    /// The user is not authenticated; they must run `lpcli login` first.
    #[error("Not authenticated. Run `lpcli login` to authenticate with Launchpad.")]
    NotAuthenticated,

    /// The requested resource was not found on Launchpad (HTTP 404).
    #[error("Not found: {0}")]
    NotFound(String),

    /// A successful API response had no matching local result.
    #[error("Not found: {0}")]
    LocalNotFound(String),

    /// The authenticated user does not have permission to perform the
    /// requested operation.
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// A configuration error (e.g. malformed credential file).
    #[error("Configuration error: {0}")]
    Config(String),

    /// A FIDO2 security-key operation failed (no key found, USB permissions,
    /// wrong PIN, unsupported key, and so on).
    #[error("Security key error: {0}")]
    HardwareKey(String),

    /// The user or the security key cancelled the operation.
    #[error("Security-key operation cancelled.")]
    HardwareKeyCancelled,

    /// The security key timed out waiting for the user (for example, no touch).
    #[error("Security-key operation timed out.")]
    HardwareKeyTimeout,

    /// A hardware-bound grant or access token could not be issued, stored or
    /// minted.
    #[error("Hardware-bound token error: {0}")]
    HardwareToken(String),

    /// Launchpad rejected the hardware-bound refresh grant during the token
    /// exchange (HTTP 401, 403 or 410): it was revoked, expired, or is bound
    /// to a different security key.
    #[error("Launchpad rejected the hardware-bound grant; check the grant and security key.")]
    HardwareTokenGrantRejected,

    /// A command-line or request value is invalid.
    #[error("{0}")]
    Usage(String),

    /// Some parts of a multi-part operation succeeded and others failed.
    ///
    /// The message lists the failures. Launchpad may already reflect the
    /// successful parts.
    #[error("{0}")]
    Partial(String),

    /// Any other error with an arbitrary message.
    #[error("{0}")]
    Other(String),
}

/// Stable error categories in lpcli's JSON contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    General,
    Usage,
    NotAuthenticated,
    NotFound,
    PermissionDenied,
    Network,
    Partial,
}

impl ErrorKind {
    /// The process exit code for this category.
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::General => 1,
            Self::Usage => 2,
            Self::NotAuthenticated => 3,
            Self::NotFound => 4,
            Self::PermissionDenied => 5,
            Self::Network => 6,
            Self::Partial => 7,
        }
    }

    /// The stable `error.kind` value in lpcli's JSON contract.
    pub const fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Usage => "usage",
            Self::NotAuthenticated => "not_authenticated",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::Network => "network",
            Self::Partial => "partial",
        }
    }
}

impl LpError {
    /// Classify this error for exit codes and structured errors.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Http(_)
            | Self::Timeout(_)
            | Self::Connect(_)
            | Self::Tls(_)
            | Self::RateLimit { .. } => ErrorKind::Network,
            Self::Api { status: 401, .. }
            | Self::NotAuthenticated
            | Self::HardwareTokenGrantRejected => ErrorKind::NotAuthenticated,
            Self::Api { status: 403, .. } | Self::Unauthorized(_) => ErrorKind::PermissionDenied,
            Self::Api {
                status: 404 | 410, ..
            }
            | Self::NotFound(_)
            | Self::LocalNotFound(_) => ErrorKind::NotFound,
            Self::Api {
                status: 408 | 429 | 500..=599,
                ..
            } => ErrorKind::Network,
            Self::Usage(_) => ErrorKind::Usage,
            Self::Partial(_) => ErrorKind::Partial,
            Self::Api { .. }
            | Self::Json(_)
            | Self::OAuth(_)
            | Self::Keyring(_)
            | Self::Io(_)
            | Self::Url(_)
            | Self::Config(_)
            | Self::HardwareKey(_)
            | Self::HardwareKeyCancelled
            | Self::HardwareKeyTimeout
            | Self::HardwareToken(_)
            | Self::Other(_) => ErrorKind::General,
        }
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        self.kind().exit_code()
    }

    /// The HTTP status represented by this error, when known.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            Self::RateLimit { .. } => Some(429),
            Self::NotFound(_) => Some(404),
            Self::LocalNotFound(_) => None,
            _ => None,
        }
    }

    /// A short recovery hint, when one applies.
    pub fn hint(&self) -> Option<&'static str> {
        match self.kind() {
            ErrorKind::NotAuthenticated => {
                Some("Run `lpcli login`, or use --public for public data.")
            }
            ErrorKind::PermissionDenied => {
                Some("Check that the account has the required Launchpad permissions.")
            }
            ErrorKind::Network => Some("Check the network connection and retry later."),
            ErrorKind::Partial => Some("Check which changes were applied before retrying."),
            _ => None,
        }
    }
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
    fn keyring_error_display() {
        let err = LpError::Keyring("secret service unavailable".to_string());
        assert!(err.to_string().contains("secret service unavailable"));
    }

    #[test]
    fn hardware_errors_display() {
        assert!(
            LpError::HardwareKey("No USB FIDO2 security key found.".to_string())
                .to_string()
                .contains("No USB FIDO2 security key found.")
        );
        assert!(
            LpError::HardwareKeyTimeout
                .to_string()
                .contains("timed out")
        );
        assert!(
            LpError::HardwareTokenGrantRejected
                .to_string()
                .contains("rejected")
        );
    }

    #[test]
    fn every_error_variant_has_a_stable_kind_and_exit_code() {
        let io = || std::io::Error::other("disk");
        let json = || serde_json::from_str::<serde_json::Value>("x").unwrap_err();
        let cases = vec![
            (LpError::Timeout("t".into()), "network", 6),
            (LpError::Connect("c".into()), "network", 6),
            (LpError::Tls("t".into()), "network", 6),
            (
                LpError::RateLimit {
                    retry_after_secs: Some(1),
                },
                "network",
                6,
            ),
            (
                LpError::Api {
                    status: 401,
                    message: "x".into(),
                },
                "not_authenticated",
                3,
            ),
            (
                LpError::Api {
                    status: 403,
                    message: "x".into(),
                },
                "permission_denied",
                5,
            ),
            (
                LpError::Api {
                    status: 404,
                    message: "x".into(),
                },
                "not_found",
                4,
            ),
            (
                LpError::Api {
                    status: 410,
                    message: "x".into(),
                },
                "not_found",
                4,
            ),
            (
                LpError::Api {
                    status: 408,
                    message: "x".into(),
                },
                "network",
                6,
            ),
            (
                LpError::Api {
                    status: 503,
                    message: "x".into(),
                },
                "network",
                6,
            ),
            (
                LpError::Api {
                    status: 400,
                    message: "x".into(),
                },
                "general",
                1,
            ),
            (LpError::Json(json()), "general", 1),
            (LpError::OAuth("o".into()), "general", 1),
            (LpError::Keyring("k".into()), "general", 1),
            (LpError::Io(io()), "general", 1),
            (LpError::Url(url::ParseError::EmptyHost), "general", 1),
            (LpError::NotAuthenticated, "not_authenticated", 3),
            (LpError::NotFound("n".into()), "not_found", 4),
            (LpError::LocalNotFound("local miss".into()), "not_found", 4),
            (LpError::Unauthorized("u".into()), "permission_denied", 5),
            (LpError::Config("c".into()), "general", 1),
            (LpError::HardwareKey("h".into()), "general", 1),
            (LpError::HardwareKeyCancelled, "general", 1),
            (LpError::HardwareKeyTimeout, "general", 1),
            (LpError::HardwareToken("h".into()), "general", 1),
            (LpError::HardwareTokenGrantRejected, "not_authenticated", 3),
            (LpError::Usage("bad value".into()), "usage", 2),
            (LpError::Partial("p".into()), "partial", 7),
            (LpError::Other("o".into()), "general", 1),
        ];
        for (error, kind, code) in cases {
            assert_eq!(
                (error.kind().name(), error.exit_code()),
                (kind, code),
                "{error:?}"
            );
        }
        assert_eq!(ErrorKind::Usage.exit_code(), 2);
        assert_eq!(LpError::NotFound("remote".into()).http_status(), Some(404));
        assert_eq!(LpError::LocalNotFound("local".into()).http_status(), None);
    }

    #[test]
    fn only_http_not_found_has_http_status_in_structured_errors() {
        assert_eq!(
            LpError::NotFound("remote resource".into()).http_status(),
            Some(404)
        );
        assert_eq!(
            LpError::LocalNotFound("no matching result".into()).http_status(),
            None
        );
    }

    #[tokio::test]
    async fn reqwest_http_errors_are_network_failures() {
        let error = reqwest::Client::new()
            .get("http://[")
            .send()
            .await
            .unwrap_err();
        assert_eq!(LpError::Http(error).exit_code(), 6);
    }

    #[test]
    fn config_error_display() {
        let err = LpError::Config("missing consumer_key".to_string());
        assert!(err.to_string().contains("missing consumer_key"));
    }
}
