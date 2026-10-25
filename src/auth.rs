//! OAuth 1.0a authentication for the Launchpad API.
//!
//! Launchpad uses OAuth 1.0a for API authentication.  This module handles:
//!
//! * Obtaining a request token from Launchpad.
//! * Directing the user to authorise the token via their browser.
//! * Exchanging the authorised request token for an access token.
//! * Persisting access tokens to the operating system keyring, with a
//!   plaintext file fallback when no keyring is available.
//! * Signing outgoing HTTP requests with the stored access token.
//!
//! Credentials are stored in the operating system keyring by default
//! (Secret Service on Linux, Keychain on macOS, Credential Manager on
//! Windows).  If the keyring is unavailable or cannot be used, lpcli falls
//! back to `~/.config/lpcli/credentials.toml`.
//!
//! # Launchpad OAuth endpoints
//!
//! | Purpose | URL |
//! |---------|-----|
//! | Request token | `https://launchpad.net/+request-token` |
//! | Authorise | `https://launchpad.net/+authorize-token` |
//! | Access token | `https://launchpad.net/+access-token` |

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{LpError, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const LAUNCHPAD_REQUEST_TOKEN_URL: &str = "https://launchpad.net/+request-token";
const LAUNCHPAD_AUTHORIZE_URL: &str = "https://launchpad.net/+authorize-token";
const LAUNCHPAD_ACCESS_TOKEN_URL: &str = "https://launchpad.net/+access-token";

/// Application name presented to Launchpad during OAuth negotiation.
pub const CONSUMER_KEY: &str = "lpcli";

/// Service name used for the lpcli entry in the operating system keyring.
const KEYRING_SERVICE: &str = "lpcli";

/// Account name used for the lpcli entry in the operating system keyring.
const KEYRING_USER: &str = "launchpad";

// ---------------------------------------------------------------------------
// Credential types
// ---------------------------------------------------------------------------

/// An OAuth 1.0a access token and its associated secret.
///
/// The [`std::fmt::Debug`] implementation is redacted so that accidental
/// `{:?}` formatting (debug logs, panic messages, error output) can never
/// expose the token or its secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessToken {
    /// The OAuth access token string.
    pub token: String,
    /// The OAuth access token secret.
    pub secret: String,
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessToken")
            .field("token", &"[redacted]")
            .field("secret", &"[redacted]")
            .finish()
    }
}

/// The full set of credentials persisted on disk.
///
/// The [`std::fmt::Debug`] implementation is redacted so that accidental
/// `{:?}` formatting can never expose the access token or its secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// The OAuth consumer key (application identifier).
    pub consumer_key: String,
    /// The access token obtained after a successful login.
    pub access_token: AccessToken,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("consumer_key", &self.consumer_key)
            .field("access_token", &"[redacted]")
            .finish()
    }
}

impl Credentials {
    /// Create a new [`Credentials`] value.
    pub fn new(
        consumer_key: impl Into<String>,
        token: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        Self {
            consumer_key: consumer_key.into(),
            access_token: AccessToken {
                token: token.into(),
                secret: secret.into(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Credential persistence
// ---------------------------------------------------------------------------

/// Returns the path to the fallback credentials file.
///
/// Defaults to `~/.config/lpcli/credentials.toml`.
pub fn credentials_path() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| LpError::Config("Cannot determine config directory".to_string()))?;
    Ok(base.join("lpcli").join("credentials.toml"))
}

/// Return the OS keyring entry used to store lpcli credentials.
fn keyring_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

/// Load credentials, preferring the OS keyring and falling back to the
/// plaintext file when no usable keyring is available.
///
/// Returns `Err(LpError::NotAuthenticated)` when neither store contains
/// credentials.
pub fn load_credentials() -> Result<Credentials> {
    if let Ok(Some(creds)) = load_from_keyring() {
        // The keyring is authoritative; remove any stale plaintext copy.
        if let Ok(path) = credentials_path() {
            let _ = delete_credentials_file(&path);
        }
        return Ok(creds);
    }

    let path = credentials_path()?;
    if !path.exists() {
        return Err(LpError::NotAuthenticated);
    }

    let creds = load_credentials_from_file(&path)?;

    // Migrate legacy plaintext credentials into the keyring when possible.
    // If the keyring is unavailable, the file remains the source of truth.
    if let Ok(entry) = keyring_entry()
        && let Ok(serialized) = serialize_credentials(&creds)
        && entry.set_password(&serialized).is_ok()
    {
        let _ = std::fs::remove_file(&path);
    }

    Ok(creds)
}

/// Load credentials from the OS keyring.
///
/// Returns `Ok(None)` when the keyring is available but has no lpcli entry.
fn load_from_keyring() -> Result<Option<Credentials>> {
    let entry = keyring_entry().map_err(|e| LpError::Keyring(e.to_string()))?;
    match entry.get_password() {
        Ok(raw) => parse_credentials(&raw).map(Some),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(LpError::Keyring(e.to_string())),
    }
}

/// Load credentials from the plaintext fallback file.
fn load_credentials_from_file(path: &Path) -> Result<Credentials> {
    let raw = std::fs::read_to_string(path)?;
    parse_credentials(&raw)
}

/// Parse the TOML representation of credentials.
///
/// The `toml` crate's error `Display` embeds the offending line of the input,
/// which for a credentials file may contain the OAuth token or secret.  Parse
/// errors therefore report only the message and location, never file contents.
fn parse_credentials(raw: &str) -> Result<Credentials> {
    toml::from_str(raw).map_err(|e| {
        let location = e
            .span()
            .and_then(|span| line_col(raw, span.start))
            .map(|(line, column)| format!(" at line {line}, column {column}"))
            .unwrap_or_default();
        LpError::Config(format!(
            "Failed to parse credentials{location}: {}",
            e.message()
        ))
    })
}

/// Convert a byte offset into a 1-based `(line, column)` pair.
fn line_col(input: &str, offset: usize) -> Option<(usize, usize)> {
    let mut line = 1usize;
    let mut column = 1usize;
    for (idx, ch) in input.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Some((line, column))
}

/// Serialise credentials to the TOML representation used by both stores.
fn serialize_credentials(creds: &Credentials) -> Result<String> {
    toml::to_string_pretty(creds)
        .map_err(|e| LpError::Config(format!("Failed to serialise credentials: {e}")))
}

/// Persist credentials, preferring the OS keyring and falling back to the
/// plaintext file when no usable keyring is available.
pub fn save_credentials(creds: &Credentials) -> Result<()> {
    let serialized = serialize_credentials(creds)?;

    // Prefer the OS keyring.
    if let Ok(entry) = keyring_entry()
        && entry.set_password(&serialized).is_ok()
    {
        // Remove any legacy plaintext copy so credentials are not duplicated.
        if let Ok(path) = credentials_path() {
            delete_credentials_file(&path)?;
        }
        return Ok(());
    }

    // Fall back to the plaintext file.
    save_credentials_to_file(&serialized, &credentials_path()?)
}

/// Write the serialised credentials to the fallback file.
fn save_credentials_to_file(serialized: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(serialized.as_bytes())?;
        // `mode` only applies when the file is created; tighten the
        // permissions explicitly so a pre-existing file is never left
        // world- or group-readable after a rewrite.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, serialized)?;
    }

    Ok(())
}

/// Delete credentials from both the OS keyring and the fallback file.
pub fn delete_credentials() -> Result<()> {
    let keyring_result = delete_from_keyring();
    let file_result = delete_credentials_file(&credentials_path()?);

    match keyring_result {
        Ok(())
        | Err(keyring::Error::NoEntry)
        | Err(keyring::Error::NoDefaultStore)
        | Err(keyring::Error::NotSupportedByStore(_)) => file_result,
        Err(e) => Err(LpError::Keyring(e.to_string())),
    }
}

/// Delete the lpcli entry from the OS keyring.
fn delete_from_keyring() -> keyring::Result<()> {
    keyring_entry()?.delete_credential()
}

/// Delete the fallback credentials file, treating a missing file as success.
fn delete_credentials_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LpError::Io(e)),
    }
}

// ---------------------------------------------------------------------------
// OAuth 1.0a signing
// ---------------------------------------------------------------------------

/// Generate a random OAuth nonce (32 hex characters).
pub fn generate_nonce() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

/// Return the current Unix timestamp as a string.
pub fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Percent-encode a string according to RFC 3986 (as required by OAuth 1.0a).
///
/// Characters in the unreserved set — `ALPHA / DIGIT / "-" / "." / "_" / "~"` —
/// are kept as-is; all other bytes are encoded as `%XX` with uppercase hex digits.
/// This is distinct from `application/x-www-form-urlencoded` encoding, which
/// incorrectly encodes `~` as `%7E` and uses `+` for spaces instead of `%20`.
pub fn percent_encode(input: &str) -> String {
    let mut output = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

/// Build and return the `Authorization` header value for an OAuth 1.0a signed
/// request using the PLAINTEXT signature method.
///
/// Launchpad's web service uses PLAINTEXT signing over TLS (HTTPS).  The
/// signature is `"&" + percent_encode(token_secret)` — that is, an empty
/// consumer secret, an ampersand, and the percent-encoded access token
/// secret.  When placed inside the Authorization header the `&` is itself
/// percent-encoded as `%26`, exactly as shown in Launchpad's API
/// documentation.
///
/// See: <https://documentation.ubuntu.com/launchpad/user/how-to/launchpad-api/launchpad-web-signing/>
pub fn build_auth_header(creds: &Credentials) -> Result<String> {
    let nonce = generate_nonce();
    let ts = timestamp();

    // PLAINTEXT signature: percent_encode(consumer_secret) + "&" + percent_encode(token_secret).
    // Launchpad does not use a consumer secret, so it is an empty string.
    let signature = format!("&{}", percent_encode(&creds.access_token.secret));

    // Build the Authorization header.  All values are percent-encoded per
    // RFC 5849 §3.5.1.  The signature's leading `&` becomes `%26`.
    let header = format!(
        r#"OAuth realm="https://api.launchpad.net/", oauth_consumer_key="{}", oauth_token="{}", oauth_signature_method="PLAINTEXT", oauth_timestamp="{}", oauth_nonce="{}", oauth_version="1.0", oauth_signature="{}""#,
        percent_encode(&creds.consumer_key),
        percent_encode(&creds.access_token.token),
        ts,
        nonce,
        percent_encode(&signature),
    );

    Ok(header)
}

// ---------------------------------------------------------------------------
// Login / logout flows
// ---------------------------------------------------------------------------

/// Perform the full Launchpad OAuth login flow.
///
/// 1. Obtains a request token.
/// 2. Prints the authorisation URL for the user to visit in their browser.
/// 3. Waits for the user to press Enter after authorising.
/// 4. Exchanges the request token for an access token.
/// 5. Persists the access token to disk.
pub async fn login() -> Result<Credentials> {
    let client = reqwest::Client::new();

    // Step 1 – obtain a request token.
    let resp = client
        .post(LAUNCHPAD_REQUEST_TOKEN_URL)
        .form(&[
            ("oauth_consumer_key", CONSUMER_KEY),
            ("oauth_signature_method", "PLAINTEXT"),
            ("oauth_signature", "&"),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp.text().await.unwrap_or_default();
        return Err(LpError::Api { status, message });
    }

    let body = resp.text().await?;
    let request_token = parse_oauth_response(&body, "oauth_token")?;
    let request_token_secret = parse_oauth_response(&body, "oauth_token_secret")?;

    // Step 2 – direct the user to the authorisation page.
    let auth_url = format!(
        "{}?oauth_token={}",
        LAUNCHPAD_AUTHORIZE_URL,
        percent_encode(&request_token)
    );
    println!("Please open the following URL in your browser to authorise lpcli:");
    println!("\n  {auth_url}\n");
    println!("After authorising, press Enter to continue...");

    // Step 3 – wait for the user.
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;

    // Step 4 – exchange for an access token.
    let signing_key = format!("&{}", percent_encode(&request_token_secret));
    let resp = client
        .post(LAUNCHPAD_ACCESS_TOKEN_URL)
        .form(&[
            ("oauth_consumer_key", CONSUMER_KEY),
            ("oauth_token", request_token.as_str()),
            ("oauth_signature_method", "PLAINTEXT"),
            ("oauth_signature", signing_key.as_str()),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let message = resp.text().await.unwrap_or_default();
        return Err(LpError::Api { status, message });
    }

    let body = resp.text().await?;
    let token = parse_oauth_response(&body, "oauth_token")?;
    let secret = parse_oauth_response(&body, "oauth_token_secret")?;

    let creds = Credentials::new(CONSUMER_KEY, token, secret);

    // Step 5 – persist.
    save_credentials(&creds)?;

    Ok(creds)
}

/// Remove the stored credentials, logging the user out.
pub fn logout() -> Result<()> {
    delete_credentials()?;
    println!("You have been logged out from Launchpad.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a key=value pair from an OAuth URL-encoded response body.
fn parse_oauth_response(body: &str, key: &str) -> Result<String> {
    url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| LpError::OAuth(format!("Missing '{key}' in OAuth response")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_roundtrip() {
        let creds = Credentials::new("lpcli", "my_token", "my_secret");
        assert_eq!(creds.consumer_key, "lpcli");
        assert_eq!(creds.access_token.token, "my_token");
        assert_eq!(creds.access_token.secret, "my_secret");
    }

    #[test]
    fn percent_encode_special_chars() {
        // Spaces must be %20 (RFC 3986), not + (form-urlencoded).
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("a=b&c=d"), "a%3Db%26c%3Dd");
        // ~ is an RFC 3986 unreserved character and must NOT be encoded.
        // This is critical for OAuth signatures on URLs like /~username.
        assert_eq!(percent_encode("~ubuntu"), "~ubuntu");
        assert_eq!(
            percent_encode("https://api.launchpad.net/devel/~ubuntu"),
            "https%3A%2F%2Fapi.launchpad.net%2Fdevel%2F~ubuntu"
        );
    }

    #[test]
    fn generate_nonce_length() {
        let nonce = generate_nonce();
        // 16 bytes => 32 hex chars
        assert_eq!(nonce.len(), 32);
    }

    #[test]
    fn timestamp_is_nonzero() {
        let ts: u64 = timestamp().parse().expect("timestamp should be a number");
        assert!(ts > 0);
    }

    #[test]
    fn parse_oauth_response_valid() {
        let body = "oauth_token=abc123&oauth_token_secret=xyz789";
        assert_eq!(parse_oauth_response(body, "oauth_token").unwrap(), "abc123");
        assert_eq!(
            parse_oauth_response(body, "oauth_token_secret").unwrap(),
            "xyz789"
        );
    }

    #[test]
    fn parse_oauth_response_missing_key() {
        let body = "oauth_token=abc123";
        let err = parse_oauth_response(body, "oauth_token_secret").unwrap_err();
        assert!(err.to_string().contains("oauth_token_secret"));
    }

    /// A malformed credentials file must never leak its contents (which
    /// include the OAuth token and secret) through the error message.
    #[test]
    fn parse_credentials_error_does_not_leak_secrets() {
        // Type mismatch: the `toml` error Display would embed the offending
        // line, but the reported message must not.
        let raw = "consumer_key = \"lpcli\"\n\n[access_token]\ntoken = \"SECRET-TOKEN\"\nsecret = 12345\n";
        let err = parse_credentials(raw).unwrap_err().to_string();
        assert!(!err.contains("SECRET-TOKEN"), "leaked via {err}");
        assert!(err.contains("line 5"), "msg: {err}");

        // Syntax error directly on the token line.
        let raw = "consumer_key = \"lpcli\"\n\n[access_token]\ntoken = \"SECRET-TOKEN\nsecret = \"SECRET-SECRET\"\n";
        let err = parse_credentials(raw).unwrap_err().to_string();
        assert!(!err.contains("SECRET"), "leaked via {err}");
        assert!(err.contains("line"), "msg: {err}");
    }

    /// `Debug` output for credential types must be redacted so that
    /// accidental `{:?}` formatting can never expose the token or secret.
    #[test]
    fn credential_debug_output_is_redacted() {
        let creds = Credentials::new("lpcli", "SECRET-TOKEN", "SECRET-SECRET");
        let debug = format!("{creds:?}");
        assert!(!debug.contains("SECRET"), "leaked via {debug}");
        assert!(debug.contains("consumer_key"), "msg: {debug}");

        let debug = format!("{:?}", creds.access_token);
        assert!(!debug.contains("SECRET"), "leaked via {debug}");
    }

    #[test]
    fn line_col_counts_lines_and_columns() {
        assert_eq!(line_col("abc", 0), Some((1, 1)));
        assert_eq!(line_col("abc", 2), Some((1, 3)));
        assert_eq!(line_col("a\nbcd", 4), Some((2, 3)));
        // Offsets past the end clamp to the input length.
        assert_eq!(line_col("a\n", 99), Some((2, 1)));
    }

    #[test]
    fn credentials_serialize_parse_roundtrip() {
        let creds = Credentials::new("lpcli", "token", "secret");
        let serialized = serialize_credentials(&creds).unwrap();
        let parsed = parse_credentials(&serialized).unwrap();
        assert_eq!(parsed, creds);
    }

    #[test]
    fn save_and_load_credentials_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        let creds = Credentials::new("lpcli", "token", "secret");

        let serialized = serialize_credentials(&creds).unwrap();
        save_credentials_to_file(&serialized, &path).unwrap();

        let loaded = load_credentials_from_file(&path).unwrap();
        assert_eq!(loaded, creds);
    }

    #[cfg(unix)]
    #[test]
    fn save_credentials_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        let creds = Credentials::new("lpcli", "token", "secret");
        let serialized = serialize_credentials(&creds).unwrap();
        save_credentials_to_file(&serialized, &path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn delete_credentials_file_missing_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        assert!(delete_credentials_file(&path).is_ok());
    }

    #[test]
    fn delete_credentials_file_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(&path, "test").unwrap();
        assert!(delete_credentials_file(&path).is_ok());
        assert!(!path.exists());
    }

    #[test]
    fn build_auth_header_contains_required_fields() {
        let creds = Credentials::new("lpcli", "tok", "sec");
        let header = build_auth_header(&creds).unwrap();
        assert!(header.starts_with("OAuth realm="));
        assert!(header.contains("oauth_consumer_key=\"lpcli\""));
        assert!(header.contains("oauth_token=\"tok\""));
        assert!(header.contains("oauth_signature_method=\"PLAINTEXT\""));
        // Signature should be percent-encoded "&sec" → "%26sec"
        assert!(header.contains("oauth_signature=\"%26sec\""));
    }
}
