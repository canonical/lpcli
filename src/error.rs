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

/// Replace credential-like values in error or status text with `[redacted]`.
///
/// Redacts double- or single-quoted and unquoted values for OAuth and bearer
/// token keys (`oauth_token_secret`, `oauth_token`, `oauth_signature`,
/// `oauth_consumer_key`, `oauth_verifier`, `consumer_secret`,
/// `refresh_token_secret`, `refresh_token`, `access_token`, and quoted
/// `authorization`), as well as `Authorization:` HTTP header lines.
pub fn redact_secrets(message: &str) -> String {
    const KEYS: [&str; 9] = [
        "oauth_token_secret",
        "oauth_token",
        "oauth_signature",
        "oauth_consumer_key",
        "oauth_verifier",
        "consumer_secret",
        "refresh_token_secret",
        "refresh_token",
        "access_token",
    ];
    let mut redacted = message.to_string();
    // Maintain `lower` in lockstep with `redacted`: every edit replaces a slice
    // at valid UTF-8 boundaries with pure lowercase/mixed ASCII, so byte offsets
    // remain identical without re-lowercasing the entire string per match.
    let mut lower = redacted.to_ascii_lowercase();

    for key in KEYS {
        redact_key_values(&mut redacted, &mut lower, key, false);
    }
    redact_key_values(&mut redacted, &mut lower, "authorization", true);

    let mut search = 0;
    while let Some(offset) = lower
        .get(search..)
        .and_then(|tail| tail.find("authorization"))
    {
        let start = search + offset;
        let bytes = redacted.as_bytes();
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            search = start + "authorization".len();
            continue;
        }
        let mut cursor = start + "authorization".len();
        while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b':') {
            search = start + "authorization".len();
            continue;
        }
        let end = redacted[start..]
            .find(['\r', '\n'])
            .map_or(redacted.len(), |off| start + off);
        redacted.replace_range(start..end, "Authorization: [redacted]");
        lower.replace_range(start..end, "authorization: [redacted]");
        search = start + "Authorization: [redacted]".len();
    }

    redacted
}

fn redact_key_values(redacted: &mut String, lower: &mut String, key: &str, require_quote: bool) {
    let mut search = 0;
    while let Some(offset) = lower.get(search..).and_then(|tail| tail.find(key)) {
        let key_start = search + offset;
        let mut cursor = key_start + key.len();
        let bytes = redacted.as_bytes();
        let had_quote = if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
            cursor += 1;
            true
        } else {
            false
        };
        if require_quote && !had_quote {
            search = key_start + key.len();
            continue;
        }
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if !matches!(bytes.get(cursor), Some(b':' | b'=')) {
            search = key_start + key.len();
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
            let quote = char::from(bytes[cursor]);
            let value_start = cursor + 1;
            let mut escaped = false;
            let mut value_end = redacted.len();
            for (rel_idx, ch) in redacted[value_start..].char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    value_end = value_start + rel_idx;
                    break;
                }
            }
            redacted.replace_range(value_start..value_end, "[redacted]");
            lower.replace_range(value_start..value_end, "[redacted]");
            search = value_start + "[redacted]".len();
        } else {
            let value_start = cursor;
            let is_delim = |c: char| {
                c.is_whitespace() || matches!(c, '&' | ',' | ';' | '}' | ')' | ']' | '"' | '\'')
            };
            if let Some(after) = redacted
                .get(value_start..)
                .and_then(|s| s.strip_prefix("[redacted]"))
                && after.chars().next().is_none_or(is_delim)
            {
                search = value_start + "[redacted]".len();
                continue;
            }
            let value_end = redacted[value_start..]
                .find(is_delim)
                .map_or(redacted.len(), |off| value_start + off);
            if value_start == value_end {
                search = key_start + key.len();
            } else {
                redacted.replace_range(value_start..value_end, "[redacted]");
                lower.replace_range(value_start..value_end, "[redacted]");
                search = value_start + "[redacted]".len();
            }
        }
    }
}

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

    #[test]
    fn redact_secrets_covers_single_quotes_quoted_auth_and_verifier() {
        let input = concat!(
            "{'oauth_token_secret': 'py-secret', 'access_token': 'py-access', ",
            "\"Authorization\": \"Bearer quoted-bearer\", ",
            "'Authorization': 'Basic quoted-basic', ",
            "\"oauth_verifier\":\"ver-123\"}\n",
            "oauth_verifier=ver-form&refresh_token='café-é'\n",
            "Authorization: Bearer header-secret\r\n",
            "trailing"
        );
        let redacted = redact_secrets(input);
        for leaked in [
            "py-secret",
            "py-access",
            "quoted-bearer",
            "quoted-basic",
            "ver-123",
            "ver-form",
            "café-é",
            "header-secret",
        ] {
            assert!(!redacted.contains(leaked), "{leaked} leaked: {redacted}");
        }
        assert!(redacted.contains("'oauth_token_secret': '[redacted]'"));
        assert!(redacted.contains("'access_token': '[redacted]'"));
        assert!(redacted.contains("\"Authorization\": \"[redacted]\""));
        assert!(redacted.contains("'Authorization': '[redacted]'"));
        assert!(redacted.contains("Authorization: [redacted]\r\ntrailing"));
        // Redaction is idempotent, including inside JSON string literals.
        assert_eq!(redact_secrets(&redacted), redacted);
        let embedded = "{\"message\": \"not allowed oauth_token=rawsecret\"}";
        let embedded_redacted = redact_secrets(embedded);
        assert_eq!(
            embedded_redacted,
            "{\"message\": \"not allowed oauth_token=[redacted]\"}"
        );
        assert_eq!(redact_secrets(&embedded_redacted), embedded_redacted);

        // Truncated quoted secret and backslash + multi-byte UTF-8 stay valid.
        let truncated = "{\"oauth_token_secret\": \"partial\\é-secret";
        let out = redact_secrets(truncated);
        assert!(!out.contains("partial"));
        assert!(out.ends_with("\"[redacted]"));
    }
}
