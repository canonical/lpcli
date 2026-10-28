//! Hardware-bound OAuth API tokens.
//!
//! An ordinary [`lpcli login`](crate::auth::login) stores a long-lived OAuth
//! access token: anyone who reads it can act as you until it is revoked.  A
//! hardware-bound login stores a long-lived OAuth *refresh grant* instead.
//! Launchpad binds the grant to one of your registered USB FIDO2 security
//! keys, and lpcli must prove that it holds the key to exchange the grant for
//! a short-lived access token.  API requests are then signed with ordinary
//! OAuth 1, so the rest of lpcli is unchanged.
//!
//! This mirrors launchpadlib's `Launchpad.login_with_hardware_token`; see
//! <https://documentation.ubuntu.com/launchpad/user/how-to/launchpadlib/use-hardware-bound-api-tokens/>.
//!
//! Hardware binding protects the stored grant, not each access token: a
//! minted token is an ordinary OAuth token for its short lifetime.
//!
//! # Workflow
//!
//! 1. Log in classically and register a security key
//!    ([`register_hardware_key`]).
//! 2. Issue a grant bound to that key ([`issue_hardware_bound_oauth_token`]).
//! 3. Log in with the grant ([`login_with_hardware_token`]), which stores it.
//! 4. Use [`load_session`] and [`LaunchpadClient::with_hardware_token`] for
//!    later requests.
//! 5. Revoke the classic bootstrap token.
//!
//! # The exchange protocol
//!
//! Two endpoints on the Launchpad web root exchange a grant for a token.
//! They are authenticated by the grant itself, not OAuth-signed:
//!
//! * `POST https://launchpad.net/+hw-challenge` with `oauth_token` and
//!   `oauth_token_secret` returns a JSON `challenge_id` and WebAuthn request
//!   options under `publicKey`.
//! * `POST https://launchpad.net/+hw-access-token` with the grant, the
//!   `challenge_id` and the JSON WebAuthn `assertion` returns a form-encoded
//!   `oauth_token`, `oauth_token_secret` and ISO 8601 `expires_at`.
//!
//! Redirects are not followed and assertions are never retried.  HTTP 401,
//! 403 and 410 mean the grant was rejected
//! ([`LpError::HardwareTokenGrantRejected`]).
//!
//! # Token renewal
//!
//! An access token is reused until shortly before it expires (the smaller of
//! 60 seconds and a tenth of its lifetime), so ordinary requests do not each
//! need a touch.  Minting is serialised, so concurrent requests share one new
//! token.  Access tokens are never persisted, so each new lpcli invocation
//! needs one touch.
//!
//! # Grant storage
//!
//! See [`GrantStore`].  The stored record uses launchpadlib's version-2
//! format, so a file written by either tool can be read by the other.  It
//! holds the refresh token and secret, the account, the optional credential
//! ID, the service root, the WebAuthn origin and the consumer key.  Access
//! tokens, challenges, assertions and PINs are never stored, and private keys
//! never leave the security key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::auth::{self, Credentials};
use crate::client::{Collection, LaunchpadClient};
use crate::error::{LpError, Result};
use crate::hardware_key::{
    Assertion, AssertionRequest, Authenticator, RegistrationRequest, ResidentKey, UserVerification,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The default OAuth consumer key for lpcli's hardware-bound grants.
///
/// It differs from the classic [`auth::CONSUMER_KEY`], so revoking the
/// classic bootstrap token on Launchpad does not also revoke the grant.
pub const HARDWARE_CONSUMER_KEY: &str = "lpcli-hardware";

/// The Launchpad web service root recorded in grant records.
pub const SERVICE_ROOT: &str = "https://api.launchpad.net/";

/// The Launchpad web root: the WebAuthn origin and the token-exchange host.
pub const WEB_ROOT: &str = "https://launchpad.net";

/// Environment variable naming a file to store the grant in instead of the
/// keyring.  launchpadlib honours the same variable.
pub const CREDENTIALS_FILE_ENV: &str = "LP_HARDWARE_CREDENTIALS_FILE";

const KEYRING_USER: &str = "launchpad-hardware-token";
const FALLBACK_FILE_NAME: &str = "hardware-token.json";
const RECORD_VERSION: u32 = 2;
const RECORD_KIND: &str = "hardware-token";
const MINTING_CHALLENGE_BYTES: usize = 32;
const DEFAULT_WEBAUTHN_TIMEOUT: Duration = Duration::from_secs(60);
const MINTING_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Base64url
// ---------------------------------------------------------------------------

/// Encode bytes as unpadded base64url, as WebAuthn JSON requires.
pub fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode a non-empty base64url string with optional padding.
///
/// Like launchpadlib, this rejects the standard Base64 alphabet, excess
/// padding and non-canonical encodings.
pub fn decode_base64url(value: &str) -> Result<Vec<u8>> {
    if value.is_empty() {
        return Err(LpError::HardwareToken(
            "Invalid base64url encoding.".to_string(),
        ));
    }
    URL_SAFE_NO_PAD_INDIFFERENT
        .decode(value)
        .map_err(|_| LpError::HardwareToken("Invalid base64url encoding.".to_string()))
}

// ---------------------------------------------------------------------------
// Grants and stored credentials
// ---------------------------------------------------------------------------

/// A hardware-bound OAuth refresh grant for one Launchpad account.
///
/// The [`std::fmt::Debug`] implementation never shows the token or secret.
#[derive(Clone, PartialEq, Eq)]
pub struct HardwareTokenGrant {
    token: String,
    secret: Zeroizing<String>,
    account: String,
    credential_id: Option<String>,
}

impl std::fmt::Debug for HardwareTokenGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareTokenGrant")
            .field("token", &"[redacted]")
            .field("secret", &"[redacted]")
            .field("account", &self.account)
            .field("credential_id", &self.credential_id)
            .finish()
    }
}

impl HardwareTokenGrant {
    /// Create a grant from the `oauth_token` and `oauth_token_secret` that
    /// Launchpad issued, owned by Launchpad `account`.
    pub fn new(
        token: impl Into<String>,
        secret: impl Into<String>,
        account: impl Into<String>,
    ) -> Result<Self> {
        let grant = Self {
            token: token.into(),
            secret: Zeroizing::new(secret.into()),
            account: account.into(),
            credential_id: None,
        };
        require_text(&grant.token, "refresh token")?;
        require_text(&grant.secret, "refresh-token secret")?;
        require_text(&grant.account, "account")?;
        Ok(grant)
    }

    /// Restrict the grant to the security-key credential with this base64url
    /// WebAuthn credential ID.  This is not the numeric ID of a registered
    /// authenticator.
    ///
    /// Fails if the grant is already restricted to a different credential.
    pub fn with_credential_id(mut self, credential_id: &str) -> Result<Self> {
        let canonical = encode_base64url(&decode_base64url(credential_id)?);
        match &self.credential_id {
            Some(existing) if *existing != canonical => Err(LpError::HardwareToken(
                "The grant is already restricted to a different security-key credential."
                    .to_string(),
            )),
            _ => {
                self.credential_id = Some(canonical);
                Ok(self)
            }
        }
    }

    /// The grant's token key (its `oauth_token`), used to revoke it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The Launchpad account that owns the grant.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The base64url credential ID the grant is restricted to, if any.
    pub fn credential_id(&self) -> Option<&str> {
        self.credential_id.as_deref()
    }

    /// The issuance response, `{"oauth_token": ..., "oauth_token_secret":
    /// ...}`, as accepted by launchpadlib's `login_with_hardware_token`.
    ///
    /// The result contains the grant secret; protect it like a password.
    pub fn to_issuance_json(&self) -> Zeroizing<String> {
        Zeroizing::new(
            json!({
                "oauth_token": self.token,
                "oauth_token_secret": self.secret.as_str(),
            })
            .to_string(),
        )
    }

    fn secret(&self) -> &str {
        &self.secret
    }
}

/// A grant together with the consumer and Launchpad instance it belongs to:
/// everything that is persisted for a hardware-bound login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareTokenCredentials {
    grant: HardwareTokenGrant,
    consumer: String,
    service_root: String,
    origin: String,
}

/// The persisted record, in launchpadlib's version-2 format.  Fields are in
/// alphabetical order to match launchpadlib's `sort_keys=True` output.
#[derive(Serialize, Deserialize)]
struct Record {
    account: String,
    consumer: String,
    credential_id: Option<String>,
    kind: String,
    origin: String,
    refresh_token: String,
    refresh_token_secret: String,
    service_root: String,
    version: u32,
}

impl HardwareTokenCredentials {
    /// Bind `grant` to OAuth consumer key `consumer` on production Launchpad.
    ///
    /// `consumer` must match the `consumer_key` the grant was issued for.
    pub fn new(grant: HardwareTokenGrant, consumer: impl Into<String>) -> Result<Self> {
        let consumer = consumer.into();
        require_text(&consumer, "consumer key")?;
        Ok(Self {
            grant,
            consumer,
            service_root: SERVICE_ROOT.to_string(),
            origin: WEB_ROOT.to_string(),
        })
    }

    /// The refresh grant.
    pub fn grant(&self) -> &HardwareTokenGrant {
        &self.grant
    }

    /// The OAuth consumer key that minted access tokens are used with.
    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    /// The Launchpad web service root.
    pub fn service_root(&self) -> &str {
        &self.service_root
    }

    /// The WebAuthn origin.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Serialise to the launchpadlib-compatible JSON record.  The result
    /// contains the grant secret.
    pub fn to_json(&self) -> Result<Zeroizing<String>> {
        let record = Record {
            account: self.grant.account.clone(),
            consumer: self.consumer.clone(),
            credential_id: self.grant.credential_id.clone(),
            kind: RECORD_KIND.to_string(),
            origin: self.origin.clone(),
            refresh_token: self.grant.token.clone(),
            refresh_token_secret: self.grant.secret.to_string(),
            service_root: self.service_root.clone(),
            version: RECORD_VERSION,
        };
        let serialized = Zeroizing::new(serde_json::to_string(&record)?);
        drop(Zeroizing::new(record.refresh_token_secret));
        Ok(serialized)
    }

    /// Parse a launchpadlib-compatible JSON record.
    ///
    /// Errors never include the record's contents, which hold the secret.
    pub fn from_json(raw: &str) -> Result<Self> {
        let invalid = || LpError::HardwareToken("Invalid hardware credential record.".to_string());

        let record: Record = serde_json::from_str(raw).map_err(|_| invalid())?;
        let secret = Zeroizing::new(record.refresh_token_secret);
        if record.version != RECORD_VERSION || record.kind != RECORD_KIND {
            return Err(invalid());
        }
        if record.service_root != SERVICE_ROOT {
            return Err(LpError::HardwareToken(format!(
                "The stored grant is for {}, but lpcli only supports {SERVICE_ROOT}.",
                record.service_root
            )));
        }

        let mut grant =
            HardwareTokenGrant::new(record.refresh_token, secret.as_str(), record.account)
                .map_err(|_| invalid())?;
        if let Some(credential_id) = record.credential_id {
            grant = grant
                .with_credential_id(&credential_id)
                .map_err(|_| invalid())?;
        }
        require_text(&record.consumer, "consumer key").map_err(|_| invalid())?;

        Ok(Self {
            grant,
            consumer: record.consumer,
            service_root: record.service_root,
            origin: validate_origin(&record.origin).map_err(|_| invalid())?,
        })
    }
}

fn require_text(value: &str, what: &str) -> Result<()> {
    if value.is_empty() {
        Err(LpError::HardwareToken(format!(
            "The {what} must not be empty."
        )))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Grant storage
// ---------------------------------------------------------------------------

/// Where the hardware-bound grant is stored.
///
/// lpcli stores one grant at a time.  The grant is useless without its
/// security key, but it is still a secret: protect a grant file like a
/// password.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GrantStore {
    /// The operating system keyring, falling back to
    /// `~/.config/lpcli/hardware-token.json` when no keyring is usable.
    #[default]
    Keyring,
    /// A file, written atomically and readable only by its owner.
    File(PathBuf),
    /// Memory only: nothing is loaded or saved.
    Memory,
}

/// Where a grant was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredIn {
    /// The operating system keyring.
    Keyring,
    /// A file.
    File(PathBuf),
    /// Nowhere: the grant only lives in memory.
    Memory,
}

impl std::fmt::Display for StoredIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyring => f.write_str("the system keyring"),
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Memory => f.write_str("memory only (not persisted)"),
        }
    }
}

impl GrantStore {
    /// The store selected by the environment, like launchpadlib:
    ///
    /// * [`CREDENTIALS_FILE_ENV`] names a file to use instead of the keyring.
    /// * Under `sudo`, the grant is kept in memory only, because the
    ///   invoking user's keyring is not the right place for root's grant.
    /// * Otherwise, the keyring.
    pub fn from_env() -> Self {
        match std::env::var_os(CREDENTIALS_FILE_ENV) {
            Some(path) if !path.is_empty() => Self::File(expand_home(PathBuf::from(path))),
            _ if is_sudo() => Self::Memory,
            _ => Self::Keyring,
        }
    }

    /// Load the stored grant, if there is one.
    pub fn load(&self) -> Result<Option<HardwareTokenCredentials>> {
        match self {
            Self::Keyring => {
                if let Some(raw) = keyring_read() {
                    return HardwareTokenCredentials::from_json(&raw).map(Some);
                }
                load_file(&fallback_path()?)
            }
            Self::File(path) => load_file(path),
            Self::Memory => Ok(None),
        }
    }

    /// Save `credentials`, replacing any stored grant.
    pub fn save(&self, credentials: &HardwareTokenCredentials) -> Result<StoredIn> {
        let serialized = credentials.to_json()?;
        match self {
            Self::Keyring => {
                if let Ok(entry) = auth::keyring_entry_for(KEYRING_USER)
                    && entry.set_password(&serialized).is_ok()
                {
                    delete_file(&fallback_path()?)?;
                    return Ok(StoredIn::Keyring);
                }
                let path = fallback_path()?;
                write_private_file(&path, serialized.as_bytes())?;
                Ok(StoredIn::File(path))
            }
            Self::File(path) => {
                write_private_file(path, serialized.as_bytes())?;
                Ok(StoredIn::File(path.clone()))
            }
            Self::Memory => Ok(StoredIn::Memory),
        }
    }

    /// Delete the stored grant.  Deleting a missing grant succeeds.
    ///
    /// This only removes the local copy; use
    /// [`revoke_hardware_bound_oauth_token`] to revoke it on Launchpad.
    pub fn delete(&self) -> Result<()> {
        match self {
            Self::Keyring => {
                let keyring_result = auth::keyring_entry_for(KEYRING_USER)
                    .and_then(|entry| entry.delete_credential());
                delete_file(&fallback_path()?)?;
                match keyring_result {
                    Ok(())
                    | Err(keyring_core::Error::NoEntry)
                    | Err(keyring_core::Error::NoDefaultStore)
                    | Err(keyring_core::Error::NotSupportedByStore(_)) => Ok(()),
                    Err(e) => Err(LpError::Keyring(e.to_string())),
                }
            }
            Self::File(path) => delete_file(path),
            Self::Memory => Ok(()),
        }
    }
}

fn keyring_read() -> Option<Zeroizing<String>> {
    auth::keyring_entry_for(KEYRING_USER)
        .ok()?
        .get_password()
        .ok()
        .map(Zeroizing::new)
}

fn fallback_path() -> Result<PathBuf> {
    Ok(auth::config_dir()?.join(FALLBACK_FILE_NAME))
}

fn is_sudo() -> bool {
    ["SUDO_USER", "SUDO_UID", "SUDO_GID"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

fn expand_home(path: PathBuf) -> PathBuf {
    match (path.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path,
    }
}

fn load_file(path: &Path) -> Result<Option<HardwareTokenCredentials>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => HardwareTokenCredentials::from_json(&Zeroizing::new(raw)).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LpError::Io(e)),
    }
}

fn delete_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LpError::Io(e)),
    }
}

/// Write `data` to `path` atomically, readable only by the owner: write a
/// temporary file in the same directory, flush it, then rename it over
/// `path`, so a crash never leaves a truncated grant behind.
fn write_private_file(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| LpError::Config(format!("Invalid grant file path: {}", path.display())))?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        hex::encode(rand::random::<[u8; 8]>())
    ));

    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(LpError::Io)
}

// ---------------------------------------------------------------------------
// WebAuthn helpers
// ---------------------------------------------------------------------------

/// Check that `origin` is a bare HTTPS origin and return it without a
/// trailing slash.
fn validate_origin(origin: &str) -> Result<String> {
    let invalid = || LpError::HardwareToken("The WebAuthn origin must be an HTTPS origin.".into());
    let url = url::Url::parse(origin).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(origin.trim_end_matches('/').to_string())
}

/// Whether `rp_id` is a valid WebAuthn relying party ID for `origin`: the
/// origin's host or a registrable suffix of it.
///
/// There is no public-suffix list here, so bare top-level domains are
/// rejected as a conservative approximation.
fn verify_rp_id(rp_id: &str, origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if url.scheme() != "https" || rp_id.is_empty() {
        return false;
    }
    host == rp_id || (rp_id.contains('.') && host.ends_with(&format!(".{rp_id}")))
}

/// Build the WebAuthn `clientDataJSON` exactly as python-fido2 does, so
/// Launchpad sees the same bytes from lpcli and launchpadlib.
fn client_data_json(kind: &str, challenge: &[u8], origin: &str) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct CollectedClientData<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
        challenge: String,
        origin: &'a str,
        #[serde(rename = "crossOrigin")]
        cross_origin: bool,
    }

    Ok(serde_json::to_vec(&CollectedClientData {
        kind,
        challenge: encode_base64url(challenge),
        origin,
        cross_origin: false,
    })?)
}

/// The WebAuthn `PublicKeyCredential` JSON for an assertion.
fn assertion_json(assertion: &Assertion, client_data: &[u8]) -> Value {
    let id = encode_base64url(&assertion.credential_id);
    json!({
        "id": id,
        "rawId": id,
        "type": "public-key",
        "response": {
            "clientDataJSON": encode_base64url(client_data),
            "authenticatorData": encode_base64url(&assertion.authenticator_data),
            "signature": encode_base64url(&assertion.signature),
            "userHandle": assertion.user_handle.as_deref().map(encode_base64url),
        },
    })
}

/// The parts of WebAuthn request options used for minting.
#[derive(Debug)]
struct RequestOptions {
    challenge: Vec<u8>,
    rp_id: String,
    timeout: Duration,
    allow_credentials: Vec<Vec<u8>>,
    user_verification: UserVerification,
}

fn parse_request_options(envelope: &Value) -> Result<RequestOptions> {
    let malformed = || LpError::HardwareToken("Malformed WebAuthn request options.".to_string());

    let data = envelope
        .get("publicKey")
        .and_then(Value::as_object)
        .ok_or_else(malformed)?;
    let challenge = data
        .get("challenge")
        .and_then(Value::as_str)
        .and_then(|s| decode_base64url(s).ok())
        .ok_or_else(malformed)?;
    if challenge.len() != MINTING_CHALLENGE_BYTES {
        return Err(LpError::HardwareToken(format!(
            "The minting challenge must contain {MINTING_CHALLENGE_BYTES} bytes."
        )));
    }
    let rp_id = data
        .get("rpId")
        .and_then(Value::as_str)
        .filter(|rp_id| !rp_id.is_empty())
        .ok_or_else(malformed)?
        .to_string();
    let allow_credentials = match data.get("allowCredentials") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => parse_credential_descriptors(value).ok_or_else(malformed)?,
    };

    Ok(RequestOptions {
        challenge,
        rp_id,
        timeout: parse_timeout(data.get("timeout")).ok_or_else(malformed)?,
        allow_credentials,
        user_verification: parse_user_verification(data.get("userVerification"))?,
    })
}

/// The parts of WebAuthn creation options used for registration.
#[derive(Debug)]
struct CreationOptions {
    request: RegistrationRequest,
    challenge: Vec<u8>,
}

fn parse_creation_options(envelope: &Value) -> Result<CreationOptions> {
    let malformed = || {
        LpError::HardwareKey("Launchpad returned malformed WebAuthn creation options.".to_string())
    };
    let text = |value: &Value, key: &str| -> Option<String> {
        value.get(key).and_then(Value::as_str).map(str::to_string)
    };

    let data = envelope
        .get("publicKey")
        .filter(|d| d.is_object())
        .ok_or_else(malformed)?;
    let rp = data.get("rp").ok_or_else(malformed)?;
    let user = data.get("user").ok_or_else(malformed)?;
    let challenge = text(data, "challenge")
        .and_then(|c| decode_base64url(&c).ok())
        .ok_or_else(malformed)?;
    let algorithms = data
        .get("pubKeyCredParams")
        .and_then(Value::as_array)
        .and_then(|params| {
            params
                .iter()
                .filter(|param| param.get("type").and_then(Value::as_str) == Some("public-key"))
                .map(|param| param.get("alg").and_then(Value::as_i64))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|algorithms| !algorithms.is_empty())
        .ok_or_else(malformed)?;
    let exclude_credentials = match data.get("excludeCredentials") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => parse_credential_descriptors(value).ok_or_else(malformed)?,
    };

    let selection = data
        .get("authenticatorSelection")
        .cloned()
        .unwrap_or(Value::Null);
    let resident_key = match selection.get("residentKey").and_then(Value::as_str) {
        Some(value) => ResidentKey::from_webauthn(value).ok_or_else(malformed)?,
        None if selection.get("requireResidentKey").and_then(Value::as_bool) == Some(true) => {
            ResidentKey::Required
        }
        None => ResidentKey::Discouraged,
    };

    Ok(CreationOptions {
        request: RegistrationRequest {
            rp_id: text(rp, "id")
                .filter(|id| !id.is_empty())
                .ok_or_else(malformed)?,
            rp_name: text(rp, "name").ok_or_else(malformed)?,
            user_id: text(user, "id")
                .and_then(|id| decode_base64url(&id).ok())
                .ok_or_else(malformed)?,
            user_name: text(user, "name").ok_or_else(malformed)?,
            user_display_name: text(user, "displayName").ok_or_else(malformed)?,
            client_data_hash: [0; 32],
            algorithms,
            exclude_credentials,
            resident_key,
            user_verification: parse_user_verification(selection.get("userVerification"))?,
            timeout: parse_timeout(data.get("timeout")).ok_or_else(malformed)?,
        },
        challenge,
    })
}

/// Parse a list of `{"type": "public-key", "id": <base64url>}` descriptors,
/// keeping public-key credentials only.
fn parse_credential_descriptors(value: &Value) -> Option<Vec<Vec<u8>>> {
    value
        .as_array()?
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str);
            match item.get("type").and_then(Value::as_str) {
                Some("public-key") => Some(id.and_then(|id| decode_base64url(id).ok())),
                Some(_) => None,
                None => Some(None),
            }
        })
        .collect()
}

/// Parse the WebAuthn `timeout` hint in milliseconds, defaulting to one
/// minute when the server omits it.
fn parse_timeout(value: Option<&Value>) -> Option<Duration> {
    match value {
        None => Some(DEFAULT_WEBAUTHN_TIMEOUT),
        Some(value) => value
            .as_u64()
            .filter(|&ms| ms > 0)
            .map(Duration::from_millis),
    }
}

fn parse_user_verification(value: Option<&Value>) -> Result<UserVerification> {
    match value {
        None | Some(Value::Null) => Ok(UserVerification::Preferred),
        Some(value) => value
            .as_str()
            .and_then(UserVerification::from_webauthn)
            .ok_or_else(|| LpError::HardwareToken("Invalid user-verification policy.".to_string())),
    }
}

/// Run a blocking security-key ceremony off the async runtime.
async fn run_ceremony<R: Send + 'static>(
    authenticator: &Arc<dyn Authenticator>,
    ceremony: impl FnOnce(&dyn Authenticator) -> Result<R> + Send + 'static,
) -> Result<R> {
    let authenticator = Arc::clone(authenticator);
    tokio::task::spawn_blocking(move || ceremony(authenticator.as_ref()))
        .await
        .map_err(|e| LpError::HardwareKey(format!("The security-key ceremony failed: {e}")))?
}

// ---------------------------------------------------------------------------
// Token exchange
// ---------------------------------------------------------------------------

/// A minting challenge: the opaque challenge ID and WebAuthn options.
struct MintingChallenge {
    id: u64,
    options: Value,
}

/// A short-lived access token minted from a grant.
struct MintedAccessToken {
    key: String,
    secret: Zeroizing<String>,
    expires_at: DateTime<Utc>,
}

/// Exchanges grants for access tokens using Launchpad's browser endpoints.
#[derive(Debug, Clone)]
struct MintingTransport {
    http: reqwest::Client,
    challenge_url: String,
    verify_url: String,
}

impl MintingTransport {
    fn new(web_root: &str) -> Result<Self> {
        Self::with_web_root(&validate_origin(web_root)?)
    }

    /// Build a transport without validating `web_root` (tests use a local
    /// HTTP server).
    fn with_web_root(web_root: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("lpcli/", env!("CARGO_PKG_VERSION")))
            .timeout(MINTING_HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            // A pooled connection may go stale while the user touches the
            // key, and a resent request could replay the assertion.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| LpError::HardwareToken(format!("Cannot build the HTTP client: {e}")))?;
        let web_root = web_root.trim_end_matches('/');
        Ok(Self {
            http,
            challenge_url: format!("{web_root}/+hw-challenge"),
            verify_url: format!("{web_root}/+hw-access-token"),
        })
    }

    async fn post(&self, url: &str, form: &[(&str, &str)], accept: &str) -> Result<String> {
        let response = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, accept)
            .form(form)
            .send()
            .await
            .map_err(|e| LpError::HardwareToken(format!("Minting request failed: {e}")))?;

        match response.status().as_u16() {
            200 => response
                .text()
                .await
                .map_err(|e| LpError::HardwareToken(format!("Minting response unreadable: {e}"))),
            401 | 403 | 410 => Err(LpError::HardwareTokenGrantRejected),
            code => Err(LpError::HardwareToken(format!(
                "Minting returned HTTP {code}."
            ))),
        }
    }

    async fn request_challenge(&self, grant: &HardwareTokenGrant) -> Result<MintingChallenge> {
        let body = self
            .post(
                &self.challenge_url,
                &[
                    ("oauth_token", grant.token()),
                    ("oauth_token_secret", grant.secret()),
                ],
                "application/json",
            )
            .await?;
        parse_challenge(&body)
    }

    async fn verify_assertion(
        &self,
        grant: &HardwareTokenGrant,
        challenge_id: u64,
        assertion: &Value,
    ) -> Result<MintedAccessToken> {
        let challenge_id = challenge_id.to_string();
        let assertion = serde_json::to_string(assertion)?;
        let body = self
            .post(
                &self.verify_url,
                &[
                    ("oauth_token", grant.token()),
                    ("oauth_token_secret", grant.secret()),
                    ("challenge_id", &challenge_id),
                    ("assertion", &assertion),
                ],
                "application/x-www-form-urlencoded",
            )
            .await?;
        parse_minted_token(&Zeroizing::new(body))
    }
}

fn parse_challenge(body: &str) -> Result<MintingChallenge> {
    let malformed = || LpError::HardwareToken("Malformed minting challenge.".to_string());

    let mut options: Value = serde_json::from_str(body).map_err(|_| malformed())?;
    let object = options.as_object_mut().ok_or_else(malformed)?;
    let id = object
        .remove("challenge_id")
        .and_then(|id| id.as_u64())
        .filter(|&id| id > 0)
        .ok_or_else(malformed)?;
    if !object.get("publicKey").is_some_and(Value::is_object) {
        return Err(malformed());
    }
    Ok(MintingChallenge { id, options })
}

fn parse_minted_token(body: &str) -> Result<MintedAccessToken> {
    let malformed = || LpError::HardwareToken("Malformed minted access token.".to_string());

    let mut fields: HashMap<String, Zeroizing<String>> = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(body.as_bytes()) {
        // Duplicate values are ambiguous, including optional fields.
        if fields
            .insert(key.into_owned(), Zeroizing::new(value.into_owned()))
            .is_some()
        {
            return Err(malformed());
        }
    }

    let mut required = |name: &str| fields.remove(name).filter(|value| !value.is_empty());
    let key = required("oauth_token").ok_or_else(malformed)?;
    let secret = required("oauth_token_secret").ok_or_else(malformed)?;
    let expires_at = required("expires_at")
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .ok_or_else(malformed)?
        .with_timezone(&Utc);

    Ok(MintedAccessToken {
        key: key.to_string(),
        secret,
        expires_at,
    })
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A minted access token and when to replace it.
struct CachedToken {
    credentials: Credentials,
    renew_at: DateTime<Utc>,
}

/// Signs Launchpad API requests with short-lived access tokens minted from a
/// hardware-bound grant.
///
/// Pass it to [`LaunchpadClient::with_hardware_token`].  Tokens are minted
/// lazily, on the first signed request, and renewed shortly before they
/// expire.
pub struct HardwareTokenSession {
    credentials: HardwareTokenCredentials,
    transport: MintingTransport,
    authenticator: Arc<dyn Authenticator>,
    cached: tokio::sync::Mutex<Option<CachedToken>>,
}

impl std::fmt::Debug for HardwareTokenSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareTokenSession")
            .field("account", &self.credentials.grant.account)
            .field("consumer", &self.credentials.consumer)
            .finish_non_exhaustive()
    }
}

impl HardwareTokenSession {
    /// Create a session for `credentials` that uses `authenticator` (for
    /// example a [`crate::hardware_key::UsbAuthenticator`]) to mint tokens.
    pub fn new(
        credentials: HardwareTokenCredentials,
        authenticator: Arc<dyn Authenticator>,
    ) -> Result<Self> {
        let transport = MintingTransport::new(WEB_ROOT)?;
        Ok(Self::with_transport(credentials, authenticator, transport))
    }

    fn with_transport(
        credentials: HardwareTokenCredentials,
        authenticator: Arc<dyn Authenticator>,
        transport: MintingTransport,
    ) -> Self {
        Self {
            credentials,
            transport,
            authenticator,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// The grant and consumer this session mints tokens for.
    pub fn credentials(&self) -> &HardwareTokenCredentials {
        &self.credentials
    }

    /// Return OAuth credentials holding a current access token, minting a
    /// new one (which needs a touch of the security key) when there is none
    /// or it is about to expire.
    pub async fn access_credentials(&self) -> Result<Credentials> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref()
            && Utc::now() < token.renew_at
        {
            return Ok(token.credentials.clone());
        }
        *cached = None;

        let minted = self.mint().await?;
        let remaining = minted.expires_at - Utc::now();
        if remaining <= TimeDelta::zero() {
            return Err(LpError::HardwareToken(
                "The server returned an already expired token.".to_string(),
            ));
        }
        // Renew shortly before expiry, but keep most of a short lifetime.
        let margin = std::cmp::min(TimeDelta::seconds(60), remaining / 10);

        let credentials = Credentials::new(
            self.credentials.consumer(),
            minted.key,
            minted.secret.as_str(),
        );
        *cached = Some(CachedToken {
            credentials: credentials.clone(),
            renew_at: minted.expires_at - margin,
        });
        Ok(credentials)
    }

    /// Discard the current access token, e.g. after Launchpad rejected it.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Run the exchange: fetch a challenge, have the key sign it and trade
    /// the assertion for an access token.
    async fn mint(&self) -> Result<MintedAccessToken> {
        let grant = &self.credentials.grant;
        let origin = self.credentials.origin();

        let challenge = self.transport.request_challenge(grant).await?;
        let options = parse_request_options(&challenge.options)?;
        if !verify_rp_id(&options.rp_id, origin) {
            return Err(LpError::HardwareToken(
                "Unexpected RP ID in the minting challenge.".to_string(),
            ));
        }

        let mut allow_credentials = options.allow_credentials;
        let wanted = grant.credential_id().map(decode_base64url).transpose()?;
        if let Some(wanted) = &wanted {
            if !allow_credentials.is_empty() && !allow_credentials.contains(wanted) {
                return Err(LpError::HardwareToken(
                    "The challenge does not permit the grant's registered key.".to_string(),
                ));
            }
            allow_credentials = vec![wanted.clone()];
        }

        let client_data = client_data_json("webauthn.get", &options.challenge, origin)?;
        let request = AssertionRequest {
            rp_id: options.rp_id,
            client_data_hash: Sha256::digest(&client_data).into(),
            allow_credentials,
            user_verification: options.user_verification,
            timeout: options.timeout,
        };
        let assertion =
            run_ceremony(&self.authenticator, move |key| key.get_assertion(&request)).await?;
        if wanted
            .as_ref()
            .is_some_and(|wanted| assertion.credential_id != *wanted)
        {
            return Err(LpError::HardwareToken(
                "The key returned a different credential.".to_string(),
            ));
        }

        self.transport
            .verify_assertion(
                grant,
                challenge.id,
                &assertion_json(&assertion, &client_data),
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Logging in
// ---------------------------------------------------------------------------

/// Options for [`login_with_hardware_token`].
///
/// With [`HardwareLogin::grant`], a newly issued grant is imported and, once
/// it has authenticated a request, stored.  Without it, the stored grant is
/// used.  There is no fallback to browser authorisation.
#[derive(Debug, Clone, Default)]
pub struct HardwareLogin {
    grant: Option<HardwareTokenGrant>,
    account: Option<String>,
    consumer: Option<String>,
    credential_id: Option<String>,
    store: GrantStore,
}

impl HardwareLogin {
    /// Log in with the stored grant, using [`GrantStore::from_env`].
    pub fn new() -> Self {
        Self {
            store: GrantStore::from_env(),
            ..Self::default()
        }
    }

    /// Import a newly issued grant instead of loading the stored one.
    pub fn grant(mut self, grant: HardwareTokenGrant) -> Self {
        self.grant = Some(grant);
        self
    }

    /// Require the grant to belong to `account`.
    pub fn account(mut self, account: impl Into<String>) -> Self {
        self.account = Some(account.into());
        self
    }

    /// The OAuth consumer key the grant was issued for.  Defaults to
    /// [`HARDWARE_CONSUMER_KEY`] for imported grants; stored grants record
    /// their own.
    pub fn consumer(mut self, consumer: impl Into<String>) -> Self {
        self.consumer = Some(consumer.into());
        self
    }

    /// Restrict the grant to one security key by its base64url WebAuthn
    /// credential ID.
    pub fn credential_id(mut self, credential_id: impl Into<String>) -> Self {
        self.credential_id = Some(credential_id.into());
        self
    }

    /// Where to load and store the grant.
    pub fn store(mut self, store: GrantStore) -> Self {
        self.store = store;
        self
    }
}

/// A successful hardware-bound login.
#[derive(Debug)]
pub struct HardwareLoginOutcome {
    /// A client signing requests with tokens minted from the grant.
    pub client: LaunchpadClient,
    /// The authenticated Launchpad account.
    pub account: String,
    /// Where the grant was saved, if it was new or changed.
    pub stored_in: Option<StoredIn>,
}

/// Log in to Launchpad with a hardware-bound grant.
///
/// The login mints an access token straight away, so expect to touch the
/// key.  A new grant is only stored once it has authenticated a request as
/// its account, so a failed import never replaces a working stored grant.
///
/// # Errors
///
/// [`LpError::HardwareToken`] when no grant is stored, or the grant belongs
/// to another account or consumer; [`LpError::HardwareTokenGrantRejected`]
/// when Launchpad refuses the grant; [`LpError::HardwareKey`] and friends
/// when the security key fails.
pub async fn login_with_hardware_token(
    login: HardwareLogin,
    authenticator: Arc<dyn Authenticator>,
) -> Result<HardwareLoginOutcome> {
    let (credentials, grant_changed) = resolve_credentials(&login)?;

    let account = credentials.grant().account().to_string();
    let session = HardwareTokenSession::new(credentials.clone(), authenticator)?;
    let client = LaunchpadClient::with_hardware_token(Arc::new(session));
    let stored_in = verify_and_store(&client, &credentials, grant_changed, &login.store).await?;

    Ok(HardwareLoginOutcome {
        client,
        account,
        stored_in,
    })
}

/// Work out which grant to log in with and whether it differs from the
/// stored one.
fn resolve_credentials(login: &HardwareLogin) -> Result<(HardwareTokenCredentials, bool)> {
    let (mut credentials, mut changed) = match &login.grant {
        Some(grant) => {
            let consumer = login.consumer.as_deref().unwrap_or(HARDWARE_CONSUMER_KEY);
            (
                HardwareTokenCredentials::new(grant.clone(), consumer)?,
                true,
            )
        }
        None => {
            let stored = login.store.load()?.ok_or_else(|| {
                LpError::HardwareToken(
                    "No hardware-bound grant is stored; import an authorized hardware-bound \
                     grant first."
                        .to_string(),
                )
            })?;
            if login
                .consumer
                .as_deref()
                .is_some_and(|consumer| consumer != stored.consumer())
            {
                return Err(LpError::HardwareToken(
                    "The stored grant was issued to a different consumer.".to_string(),
                ));
            }
            (stored, false)
        }
    };

    if login
        .account
        .as_deref()
        .is_some_and(|account| account != credentials.grant().account())
    {
        return Err(LpError::HardwareToken(
            "The grant belongs to a different account.".to_string(),
        ));
    }
    if let Some(credential_id) = &login.credential_id {
        let restricted = credentials
            .grant
            .clone()
            .with_credential_id(credential_id)?;
        changed |= restricted != credentials.grant;
        credentials.grant = restricted;
    }
    Ok((credentials, changed))
}

/// Check that the grant authenticates as its account and store it if it
/// changed.  Minting checks neither the consumer nor the account, so this
/// must happen before a new grant replaces the stored one.
async fn verify_and_store(
    client: &LaunchpadClient,
    credentials: &HardwareTokenCredentials,
    grant_changed: bool,
    store: &GrantStore,
) -> Result<Option<StoredIn>> {
    let me = current_person(client).await?;
    if me.name != credentials.grant().account() {
        return Err(LpError::HardwareToken(
            "The grant belongs to a different account.".to_string(),
        ));
    }
    grant_changed.then(|| store.save(credentials)).transpose()
}

/// Load the stored grant, if any, as a session for
/// [`LaunchpadClient::with_hardware_token`].  Nothing is minted until the
/// first signed request.
pub fn load_session(
    store: &GrantStore,
    authenticator: Arc<dyn Authenticator>,
) -> Result<Option<HardwareTokenSession>> {
    store
        .load()?
        .map(|credentials| HardwareTokenSession::new(credentials, authenticator))
        .transpose()
}

// ---------------------------------------------------------------------------
// Person API: security keys and grants
// ---------------------------------------------------------------------------

/// A FIDO2/WebAuthn security key registered with a Launchpad account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredAuthenticator {
    /// The authenticator's numeric ID.
    pub id: Option<u64>,
    /// The name its owner gave it.
    pub name: Option<String>,
    /// The attestation format used at registration.
    pub attestation_format: Option<String>,
    /// Transport hints reported at registration (e.g. `usb`).
    pub transports: Option<Vec<String>>,
    /// When it was registered.
    pub date_created: Option<DateTime<Utc>>,
    /// When it last signed a challenge.
    pub date_last_used: Option<DateTime<Utc>>,
    /// When it was revoked, if it has been.
    pub date_revoked: Option<DateTime<Utc>>,
    /// Whether it has been revoked.
    pub is_revoked: Option<bool>,
    /// API self-link, used to issue grants for and revoke it.
    pub self_link: Option<String>,
}

impl RegisteredAuthenticator {
    /// Whether the authenticator can still be used.
    pub fn is_active(&self) -> bool {
        self.is_revoked != Some(true) && self.date_revoked.is_none()
    }
}

/// An OAuth permission level for a hardware-bound grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OAuthPermission {
    /// Read public data.
    ReadNonPrivate,
    /// Read and change public data.
    ChangeNonPrivate,
    /// Read all data, including private data.
    ReadAnything,
    /// Read and change all data, including private data.
    ChangeAnything,
}

impl OAuthPermission {
    /// The permission title Launchpad expects, e.g. `Change Anything`.
    pub fn title(self) -> &'static str {
        match self {
            Self::ReadNonPrivate => "Read Non-Private Data",
            Self::ChangeNonPrivate => "Change Non-Private Data",
            Self::ReadAnything => "Read Anything",
            Self::ChangeAnything => "Change Anything",
        }
    }
}

impl std::fmt::Display for OAuthPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.title())
    }
}

/// Parameters for [`issue_hardware_bound_oauth_token`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueGrantRequest {
    /// The OAuth consumer that will log in with the grant.
    pub consumer_key: String,
    /// The `self_link` of the registered authenticator to bind it to.
    pub authenticator_link: String,
    /// The grant's permission.  It cannot exceed the permission of the
    /// credentials used to issue it.
    pub permission: OAuthPermission,
    /// A pillar name, or `distribution/sourcepackagename`, to restrict the
    /// grant to.
    pub context: Option<String>,
    /// When the grant expires.  Launchpad defaults to, and limits grants
    /// to, 365 days.
    pub date_expires: Option<DateTime<Utc>>,
}

/// The authenticated person.
#[derive(Debug, Clone, Deserialize)]
struct Person {
    name: String,
    self_link: String,
}

async fn current_person(client: &LaunchpadClient) -> Result<Person> {
    #[derive(Deserialize)]
    struct ServiceRoot {
        me_link: Option<String>,
    }

    let root: ServiceRoot = client.get("").await?;
    let me_link = root.me_link.ok_or(LpError::NotAuthenticated)?;
    client.get_url(&me_link).await
}

/// List the security keys registered with the authenticated account, most
/// recently registered first.
pub async fn list_registered_authenticators(
    client: &LaunchpadClient,
) -> Result<Vec<RegisteredAuthenticator>> {
    let me = current_person(client).await?;
    let url = format!("{}?ws.op=getRegisteredAuthenticators", me.self_link);
    Collection::fetch_all(client, &url).await
}

/// Register a USB security key with the authenticated account, naming it
/// `name`.  The user must touch the key.
///
/// The registration state lives in the server session, so the completion
/// request carries the session cookie set by the first request.
pub async fn register_hardware_key(
    client: &LaunchpadClient,
    name: &str,
    authenticator: Arc<dyn Authenticator>,
) -> Result<RegisteredAuthenticator> {
    require_text(name, "security-key name")?;
    let me = current_person(client).await?;

    let begin = client
        .post_pairs_url_raw(
            &me.self_link,
            &[("ws.op", "beginWebAuthnRegistration")],
            None,
        )
        .await?;
    let cookie = cookie_header(begin.headers());
    let envelope: Value = client.handle_response(begin).await?;

    let CreationOptions {
        mut request,
        challenge,
    } = parse_creation_options(&envelope)?;
    if !verify_rp_id(&request.rp_id, WEB_ROOT) {
        return Err(LpError::HardwareKey(
            "The RP ID does not match the Launchpad origin.".to_string(),
        ));
    }
    let client_data = client_data_json("webauthn.create", &challenge, WEB_ROOT)?;
    request.client_data_hash = Sha256::digest(&client_data).into();

    let registration =
        run_ceremony(&authenticator, move |key| key.make_credential(&request)).await?;
    let credential_id = encode_base64url(&registration.credential_id);
    let registration_json = json!({
        "id": credential_id,
        "rawId": credential_id,
        "type": "public-key",
        "response": {
            "clientDataJSON": encode_base64url(&client_data),
            "attestationObject": encode_base64url(&registration.attestation_object),
        },
    });
    // `response` is a Text parameter, which the web service decodes from
    // JSON, so the JSON registration is encoded a second time.
    let response_param = serde_json::to_string(&serde_json::to_string(&registration_json)?)?;

    let complete = client
        .post_pairs_url_raw(
            &me.self_link,
            &[
                ("ws.op", "completeWebAuthnRegistration"),
                ("response", &response_param),
                ("name", name),
            ],
            cookie.as_deref(),
        )
        .await?;
    let location = client.handle_created_location(complete).await?;
    client.get_url(&location).await
}

/// Revoke a registered security key by its `self_link`.
///
/// Grants bound to it can no longer mint access tokens, but tokens already
/// minted stay valid until they expire; revoke the grants to expire them
/// immediately.
pub async fn revoke_registered_authenticator(
    client: &LaunchpadClient,
    authenticator_link: &str,
) -> Result<()> {
    client
        .post_pairs_url_ok(authenticator_link, &[("ws.op", "revoke")])
        .await
}

/// Issue a hardware-bound refresh grant for the authenticated account.
///
/// Launchpad returns the grant secret only once: log in with the result
/// straight away, or keep it secret.
pub async fn issue_hardware_bound_oauth_token(
    client: &LaunchpadClient,
    request: &IssueGrantRequest,
) -> Result<HardwareTokenGrant> {
    require_text(&request.consumer_key, "consumer key")?;
    let me = current_person(client).await?;

    let date_expires = request.date_expires.map(|date| date.to_rfc3339());
    let mut params = vec![
        ("ws.op", "issueHardwareBoundOAuthToken"),
        ("consumer_key", request.consumer_key.as_str()),
        ("authenticator", request.authenticator_link.as_str()),
        ("permission", request.permission.title()),
    ];
    if let Some(context) = &request.context {
        params.push(("context", context));
    }
    if let Some(date_expires) = &date_expires {
        params.push(("date_expires", date_expires));
    }

    let response: Value = client.post_pairs_url(&me.self_link, &params).await?;
    let field = |name: &str| response.get(name).and_then(Value::as_str);
    match (field("oauth_token"), field("oauth_token_secret")) {
        (Some(token), Some(secret)) => HardwareTokenGrant::new(token, secret, me.name),
        _ => Err(LpError::HardwareToken(
            "Launchpad returned a malformed refresh-grant response.".to_string(),
        )),
    }
}

/// Revoke one of the authenticated account's hardware-bound grants by its
/// token key.  Access tokens already minted from it expire as well.
///
/// This does not remove a stored copy; see [`GrantStore::delete`].
pub async fn revoke_hardware_bound_oauth_token(
    client: &LaunchpadClient,
    token_key: &str,
) -> Result<()> {
    require_text(token_key, "refresh token")?;
    let me = current_person(client).await?;
    client
        .post_pairs_url_ok(
            &me.self_link,
            &[
                ("ws.op", "revokeHardwareBoundOAuthToken"),
                ("token_key", token_key),
            ],
        )
        .await
}

/// Build a `Cookie` header from a response's `Set-Cookie` headers.
fn cookie_header(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let cookies: Vec<&str> = headers
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::trim)
        .filter(|cookie| cookie.contains('='))
        .collect();
    (!cookies.is_empty()).then(|| cookies.join("; "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mockito::Matcher;

    use super::*;
    use crate::hardware_key::Registration;

    const CREDENTIAL_ID: &[u8] = b"credential-0001";

    /// A software authenticator that records requests.
    #[derive(Default)]
    struct FakeAuthenticator {
        requests: Mutex<Vec<AssertionRequest>>,
        credential_id: Option<Vec<u8>>,
    }

    impl Authenticator for FakeAuthenticator {
        fn get_assertion(&self, request: &AssertionRequest) -> Result<Assertion> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(Assertion {
                credential_id: self
                    .credential_id
                    .clone()
                    .unwrap_or_else(|| CREDENTIAL_ID.to_vec()),
                authenticator_data: b"authenticator-data".to_vec(),
                signature: b"signature".to_vec(),
                user_handle: None,
            })
        }

        fn make_credential(&self, _request: &RegistrationRequest) -> Result<Registration> {
            unreachable!("not used by these tests")
        }
    }

    fn grant() -> HardwareTokenGrant {
        HardwareTokenGrant::new("refresh-token", "refresh-secret", "example-user").unwrap()
    }

    fn challenge_body(rp_id: &str, user_verification: &str) -> String {
        json!({
            "challenge_id": 7,
            "publicKey": {
                "challenge": encode_base64url(&[0xAB; 32]),
                "rpId": rp_id,
                "timeout": 30000,
                "allowCredentials": [
                    {"type": "public-key", "id": encode_base64url(CREDENTIAL_ID)},
                ],
                "userVerification": user_verification,
            },
        })
        .to_string()
    }

    fn token_body(token: &str, lifetime: TimeDelta) -> String {
        format!(
            "oauth_token={token}&oauth_token_secret={token}-secret&expires_at={}&lp.context=None",
            url::form_urlencoded::byte_serialize((Utc::now() + lifetime).to_rfc3339().as_bytes())
                .collect::<String>()
        )
    }

    fn session(
        server: &mockito::ServerGuard,
        authenticator: Arc<FakeAuthenticator>,
        grant: HardwareTokenGrant,
    ) -> HardwareTokenSession {
        HardwareTokenSession::with_transport(
            HardwareTokenCredentials::new(grant, HARDWARE_CONSUMER_KEY).unwrap(),
            authenticator,
            MintingTransport::with_web_root(&server.url()).unwrap(),
        )
    }

    // -- Encoding and records ------------------------------------------------

    #[test]
    fn base64url_is_strict() {
        assert_eq!(encode_base64url(&[0xFB, 0xFF]), "-_8");
        assert_eq!(decode_base64url("-_8").unwrap(), vec![0xFB, 0xFF]);
        assert_eq!(decode_base64url("AQ==").unwrap(), vec![1]);
        assert!(decode_base64url("").is_err());
        assert!(decode_base64url("+/8").is_err(), "standard alphabet");
        assert!(decode_base64url("AQ===").is_err(), "excess padding");
        assert!(
            decode_base64url("AR").is_err(),
            "non-canonical trailing bits"
        );
    }

    #[test]
    fn reads_launchpadlib_records() {
        // As written by launchpadlib's json.dumps(..., sort_keys=True).
        let raw = r#"{"account": "example-user", "consumer": "hardware-client", "credential_id": null, "kind": "hardware-token", "origin": "https://launchpad.net", "refresh_token": "tok", "refresh_token_secret": "sec", "service_root": "https://api.launchpad.net/", "version": 2}"#;
        let credentials = HardwareTokenCredentials::from_json(raw).unwrap();
        assert_eq!(credentials.grant().account(), "example-user");
        assert_eq!(credentials.grant().token(), "tok");
        assert_eq!(credentials.consumer(), "hardware-client");
        assert_eq!(credentials.origin(), WEB_ROOT);
        assert_eq!(credentials.grant().credential_id(), None);
    }

    #[test]
    fn record_round_trips_with_sorted_keys() {
        let credentials = HardwareTokenCredentials::new(
            grant()
                .with_credential_id(&encode_base64url(CREDENTIAL_ID))
                .unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        let serialized = credentials.to_json().unwrap();

        let value: Value = serde_json::from_str(&serialized).unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(value["version"], 2);
        assert_eq!(value["kind"], "hardware-token");

        assert_eq!(
            HardwareTokenCredentials::from_json(&serialized).unwrap(),
            credentials
        );
    }

    #[test]
    fn invalid_records_do_not_leak_secrets() {
        for raw in [
            r#"{"version": 1, "kind": "hardware-token", "refresh_token_secret": "SECRET"}"#,
            r#"{"account": "a", "consumer": "c", "credential_id": null, "kind": "other", "origin": "https://launchpad.net", "refresh_token": "t", "refresh_token_secret": "SECRET", "service_root": "https://api.launchpad.net/", "version": 2}"#,
            r#"{"account": "a", "consumer": "c", "credential_id": null, "kind": "hardware-token", "origin": "http://launchpad.net", "refresh_token": "t", "refresh_token_secret": "SECRET", "service_root": "https://api.launchpad.net/", "version": 2}"#,
            r#"{"refresh_token_secret": 5, "SECRET": "#,
        ] {
            let err = HardwareTokenCredentials::from_json(raw)
                .unwrap_err()
                .to_string();
            assert!(!err.contains("SECRET"), "leaked via {err}");
        }
    }

    #[test]
    fn debug_output_is_redacted() {
        let debug = format!("{:?}", grant());
        assert!(!debug.contains("refresh-token"), "{debug}");
        assert!(!debug.contains("refresh-secret"), "{debug}");
        assert!(debug.contains("example-user"));
    }

    #[test]
    fn credential_id_restriction_conflicts() {
        let restricted = grant().with_credential_id("AAAA").unwrap();
        assert!(restricted.clone().with_credential_id("AAAA").is_ok());
        assert!(restricted.with_credential_id("BBBB").is_err());
        assert!(grant().with_credential_id("not base64!").is_err());
    }

    #[test]
    fn issuance_json_matches_launchpadlib() {
        let value: Value = serde_json::from_str(&grant().to_issuance_json()).unwrap();
        assert_eq!(value["oauth_token"], "refresh-token");
        assert_eq!(value["oauth_token_secret"], "refresh-secret");
    }

    // -- Storage -------------------------------------------------------------

    #[test]
    fn file_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("grant.json");
        let store = GrantStore::File(path.clone());
        assert_eq!(store.load().unwrap(), None);

        let credentials = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        assert_eq!(
            store.save(&credentials).unwrap(),
            StoredIn::File(path.clone())
        );
        assert_eq!(store.load().unwrap(), Some(credentials));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files remain");

        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.delete().unwrap();
    }

    #[test]
    fn memory_store_keeps_nothing() {
        let credentials = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        assert_eq!(
            GrantStore::Memory.save(&credentials).unwrap(),
            StoredIn::Memory
        );
        assert_eq!(GrantStore::Memory.load().unwrap(), None);
    }

    // -- WebAuthn helpers ----------------------------------------------------

    #[test]
    fn origins_must_be_bare_https() {
        assert_eq!(validate_origin("https://launchpad.net/").unwrap(), WEB_ROOT);
        assert!(validate_origin("http://launchpad.net").is_err());
        assert!(validate_origin("https://launchpad.net/path").is_err());
        assert!(validate_origin("https://user@launchpad.net").is_err());
        assert!(validate_origin("https://launchpad.net/?q=1").is_err());
    }

    #[test]
    fn rp_id_must_match_origin() {
        assert!(verify_rp_id("launchpad.net", "https://launchpad.net"));
        assert!(verify_rp_id(
            "launchpad.net",
            "https://staging.launchpad.net"
        ));
        assert!(!verify_rp_id("evil.example", "https://launchpad.net"));
        assert!(!verify_rp_id("net", "https://launchpad.net"));
        assert!(!verify_rp_id("ad.net", "https://launchpad.net"));
        assert!(!verify_rp_id("launchpad.net", "http://launchpad.net"));
        assert!(!verify_rp_id("", "https://launchpad.net"));
    }

    #[test]
    fn client_data_matches_python_fido2() {
        let data = client_data_json("webauthn.get", &[0xFB, 0xFF], WEB_ROOT).unwrap();
        assert_eq!(
            String::from_utf8(data).unwrap(),
            r#"{"type":"webauthn.get","challenge":"-_8","origin":"https://launchpad.net","crossOrigin":false}"#
        );
    }

    #[test]
    fn request_options_are_validated() {
        let envelope: Value =
            serde_json::from_str(&challenge_body("launchpad.net", "required")).unwrap();
        let options = parse_request_options(&envelope).unwrap();
        assert_eq!(options.rp_id, "launchpad.net");
        assert_eq!(options.challenge, vec![0xAB; 32]);
        assert_eq!(options.timeout, Duration::from_secs(30));
        assert_eq!(options.allow_credentials, vec![CREDENTIAL_ID.to_vec()]);
        assert_eq!(options.user_verification, UserVerification::Required);

        let short = json!({"publicKey": {"challenge": encode_base64url(&[1; 31]), "rpId": "launchpad.net"}});
        assert!(
            parse_request_options(&short)
                .unwrap_err()
                .to_string()
                .contains("32 bytes")
        );

        let bad_uv = json!({"publicKey": {"challenge": encode_base64url(&[1; 32]), "rpId": "launchpad.net", "userVerification": "always"}});
        assert!(parse_request_options(&bad_uv).is_err());

        let defaults = json!({"publicKey": {"challenge": encode_base64url(&[1; 32]), "rpId": "launchpad.net"}});
        let options = parse_request_options(&defaults).unwrap();
        assert_eq!(options.timeout, DEFAULT_WEBAUTHN_TIMEOUT);
        assert_eq!(options.user_verification, UserVerification::Preferred);
        assert!(options.allow_credentials.is_empty());
    }

    #[test]
    fn creation_options_are_parsed() {
        let envelope = json!({"publicKey": {
            "rp": {"id": "launchpad.net", "name": "Launchpad"},
            "user": {"id": encode_base64url(b"user"), "name": "example-user", "displayName": "Example"},
            "challenge": encode_base64url(&[9; 32]),
            "pubKeyCredParams": [{"type": "public-key", "alg": -7}, {"type": "public-key", "alg": -257}],
            "excludeCredentials": [{"type": "public-key", "id": encode_base64url(b"old")}],
            "authenticatorSelection": {"residentKey": "preferred", "userVerification": "discouraged"},
        }});
        let CreationOptions { request, challenge } = parse_creation_options(&envelope).unwrap();
        assert_eq!(challenge, vec![9; 32]);
        assert_eq!(request.rp_id, "launchpad.net");
        assert_eq!(request.user_id, b"user");
        assert_eq!(request.algorithms, vec![-7, -257]);
        assert_eq!(request.exclude_credentials, vec![b"old".to_vec()]);
        assert_eq!(request.resident_key, ResidentKey::Preferred);
        assert_eq!(request.user_verification, UserVerification::Discouraged);
        assert_eq!(request.timeout, DEFAULT_WEBAUTHN_TIMEOUT);

        assert!(parse_creation_options(&json!({"publicKey": {}})).is_err());
    }

    #[test]
    fn minted_tokens_are_parsed_strictly() {
        let token = parse_minted_token(&token_body("abc", TimeDelta::hours(1))).unwrap();
        assert_eq!(token.key, "abc");
        assert_eq!(token.secret.as_str(), "abc-secret");
        assert!(token.expires_at > Utc::now());

        assert!(parse_minted_token("oauth_token=a&oauth_token=b&oauth_token_secret=s").is_err());
        assert!(parse_minted_token("oauth_token=a&oauth_token_secret=s").is_err());
        assert!(
            parse_minted_token("oauth_token=a&oauth_token_secret=s&expires_at=tomorrow").is_err()
        );
    }

    #[test]
    fn challenges_need_a_positive_integer_id() {
        assert!(parse_challenge(r#"{"challenge_id": 3, "publicKey": {}}"#).is_ok());
        assert!(parse_challenge(r#"{"challenge_id": 0, "publicKey": {}}"#).is_err());
        assert!(parse_challenge(r#"{"challenge_id": true, "publicKey": {}}"#).is_err());
        assert!(parse_challenge(r#"{"challenge_id": 3}"#).is_err());
        assert!(parse_challenge("[]").is_err());
    }

    #[test]
    fn cookie_header_joins_session_cookies() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            "launchpad_tests=abc123; Path=/; Secure; HttpOnly"
                .parse()
                .unwrap(),
        );
        headers.append(
            reqwest::header::SET_COOKIE,
            "other=xyz; Path=/".parse().unwrap(),
        );
        assert_eq!(
            cookie_header(&headers).as_deref(),
            Some("launchpad_tests=abc123; other=xyz")
        );
        assert_eq!(cookie_header(&reqwest::header::HeaderMap::new()), None);
    }

    // -- Minting -------------------------------------------------------------

    #[tokio::test]
    async fn mints_once_and_reuses_the_token() {
        let mut server = mockito::Server::new_async().await;
        let challenge = server
            .mock("POST", "/+hw-challenge")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("oauth_token".into(), "refresh-token".into()),
                Matcher::UrlEncoded("oauth_token_secret".into(), "refresh-secret".into()),
            ]))
            .with_header("content-type", "application/json")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .expect(1)
            .create_async()
            .await;
        let exchange = server
            .mock("POST", "/+hw-access-token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("oauth_token".into(), "refresh-token".into()),
                Matcher::UrlEncoded("challenge_id".into(), "7".into()),
                Matcher::Regex("clientDataJSON".into()),
            ]))
            .with_body(token_body("access-1", TimeDelta::hours(1)))
            .expect(1)
            .create_async()
            .await;

        let authenticator = Arc::new(FakeAuthenticator::default());
        let session = session(&server, Arc::clone(&authenticator), grant());

        let first = session.access_credentials().await.unwrap();
        let second = session.access_credentials().await.unwrap();
        assert_eq!(first.access_token.token, "access-1");
        assert_eq!(first.access_token.secret, "access-1-secret");
        assert_eq!(first.consumer_key, HARDWARE_CONSUMER_KEY);
        assert_eq!(second, first);

        challenge.assert_async().await;
        exchange.assert_async().await;

        let requests = authenticator.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].rp_id, "launchpad.net");
        assert_eq!(requests[0].allow_credentials, vec![CREDENTIAL_ID.to_vec()]);
        assert_eq!(requests[0].user_verification, UserVerification::Discouraged);
        let expected_client_data = client_data_json("webauthn.get", &[0xAB; 32], WEB_ROOT).unwrap();
        assert_eq!(
            requests[0].client_data_hash,
            <[u8; 32]>::from(Sha256::digest(&expected_client_data))
        );
    }

    #[tokio::test]
    async fn renews_tokens_near_expiry_and_after_invalidation() {
        let mut server = mockito::Server::new_async().await;
        let _challenge = server
            .mock("POST", "/+hw-challenge")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .expect(3)
            .create_async()
            .await;
        let minted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&minted);
        let _exchange = server
            .mock("POST", "/+hw-access-token")
            .with_body_from_request(move |_| {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                // The first token is already inside its renewal margin.
                let lifetime = if n == 1 {
                    TimeDelta::milliseconds(500)
                } else {
                    TimeDelta::hours(1)
                };
                token_body(&format!("access-{n}"), lifetime).into_bytes()
            })
            .create_async()
            .await;

        let session = session(&server, Arc::new(FakeAuthenticator::default()), grant());
        // A 500 ms token is renewed 50 ms before expiry; wait past that.
        let first = session.access_credentials().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(460)).await;
        let second = session.access_credentials().await.unwrap();
        assert_eq!(first.access_token.token, "access-1");
        assert_eq!(second.access_token.token, "access-2");

        session.invalidate().await;
        let third = session.access_credentials().await.unwrap();
        assert_eq!(third.access_token.token, "access-3");
    }

    #[tokio::test]
    async fn rejected_grants_are_reported() {
        let mut server = mockito::Server::new_async().await;
        let _challenge = server
            .mock("POST", "/+hw-challenge")
            .with_status(410)
            .create_async()
            .await;
        let session = session(&server, Arc::new(FakeAuthenticator::default()), grant());
        assert!(matches!(
            session.access_credentials().await,
            Err(LpError::HardwareTokenGrantRejected)
        ));
    }

    #[tokio::test]
    async fn unexpected_rp_id_is_refused_before_touching_the_key() {
        let mut server = mockito::Server::new_async().await;
        let _challenge = server
            .mock("POST", "/+hw-challenge")
            .with_body(challenge_body("evil.example", "discouraged"))
            .create_async()
            .await;
        let authenticator = Arc::new(FakeAuthenticator::default());
        let session = session(&server, Arc::clone(&authenticator), grant());

        let err = session.access_credentials().await.unwrap_err();
        assert!(err.to_string().contains("RP ID"), "{err}");
        assert!(authenticator.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn credential_restriction_is_enforced() {
        let mut server = mockito::Server::new_async().await;
        let _challenge = server
            .mock("POST", "/+hw-challenge")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .create_async()
            .await;

        // The challenge does not allow the grant's credential.
        let other = grant()
            .with_credential_id(&encode_base64url(b"other"))
            .unwrap();
        let session_for_other = session(&server, Arc::new(FakeAuthenticator::default()), other);
        let err = session_for_other.access_credentials().await.unwrap_err();
        assert!(err.to_string().contains("does not permit"), "{err}");

        // The key answers with a credential other than the grant's.
        let restricted = grant()
            .with_credential_id(&encode_base64url(CREDENTIAL_ID))
            .unwrap();
        let authenticator = Arc::new(FakeAuthenticator {
            credential_id: Some(b"impostor".to_vec()),
            ..Default::default()
        });
        let session = session(&server, authenticator, restricted);
        let err = session.access_credentials().await.unwrap_err();
        assert!(err.to_string().contains("different credential"), "{err}");
    }

    #[tokio::test]
    async fn client_signs_api_requests_and_retries_rejected_tokens() {
        let mut server = mockito::Server::new_async().await;
        let _challenge = server
            .mock("POST", "/+hw-challenge")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .expect(2)
            .create_async()
            .await;
        let minted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&minted);
        let _exchange = server
            .mock("POST", "/+hw-access-token")
            .with_body_from_request(move |_| {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                token_body(&format!("access-{n}"), TimeDelta::hours(1)).into_bytes()
            })
            .create_async()
            .await;
        let expired = server
            .mock("GET", "/devel/bugs/1")
            .match_header(
                "authorization",
                Matcher::Regex(
                    r#"oauth_consumer_key="lpcli-hardware", oauth_token="access-1""#.into(),
                ),
            )
            .with_status(401)
            .with_body("Expired token (access-1).")
            .expect(1)
            .create_async()
            .await;
        let ok = server
            .mock("GET", "/devel/bugs/1")
            .match_header(
                "authorization",
                Matcher::Regex(r#"oauth_token="access-2""#.into()),
            )
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": 1}"#)
            .expect(1)
            .create_async()
            .await;

        let session = session(&server, Arc::new(FakeAuthenticator::default()), grant());
        let client = LaunchpadClient::with_hardware_token(Arc::new(session))
            .with_base_url(format!("{}/devel", server.url()));
        assert!(client.is_hardware_bound());

        let bug: Value = client.get("/bugs/1").await.unwrap();
        assert_eq!(bug["id"], 1);
        expired.assert_async().await;
        ok.assert_async().await;
        assert_eq!(minted.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn tokens_are_not_sent_to_other_hosts() {
        let mut minting = mockito::Server::new_async().await;
        let challenge = minting
            .mock("POST", "/+hw-challenge")
            .expect(0)
            .create_async()
            .await;
        let mut other = mockito::Server::new_async().await;
        let download = other
            .mock("GET", "/file")
            .match_header("authorization", Matcher::Missing)
            .with_body("[]")
            .create_async()
            .await;

        let session = session(&minting, Arc::new(FakeAuthenticator::default()), grant());
        let client = LaunchpadClient::with_hardware_token(Arc::new(session))
            .with_base_url(format!("{}/devel", minting.url()));
        let _: Vec<String> = client
            .get_url(&format!("{}/file", other.url()))
            .await
            .unwrap();

        download.assert_async().await;
        challenge.assert_async().await;
    }

    #[tokio::test]
    async fn login_rejects_grants_for_another_account() {
        let login = HardwareLogin::default()
            .store(GrantStore::Memory)
            .grant(grant())
            .account("someone-else");
        let err = login_with_hardware_token(login, Arc::new(FakeAuthenticator::default()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("different account"), "{err}");

        let missing = HardwareLogin::default().store(GrantStore::Memory);
        let err = login_with_hardware_token(missing, Arc::new(FakeAuthenticator::default()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No hardware-bound grant"), "{err}");
    }
}
