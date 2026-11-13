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
//! token. [`HardwareTokenSession::new`] is memory-only. [`load_session`] also
//! reuses tokens across processes through the system keyring for five minutes
//! by default; [`HardwareLogin::cache_timeout`] and [`load_session_with_cache`]
//! select 0–86400 seconds (zero disables persistence). There is no plaintext
//! access-token fallback.
//! [`clear_cached_access_token`] explicitly removes the persisted token.
//!
//! A private lock and non-secret generation marker in an ownership-checked,
//! canonical per-user directory (using the passwd home cache directory
//! `~/.cache/lpcli` independent of environment overrides, falling back to
//! `/run/user/<uid>` if home is unavailable or unwritable) coordinate
//! cooperating processes. Without either directory, caching is memory-only while
//! existing grants and uncached grant changes remain usable; coordination
//! contention or mid-login coordination changes fail safely without committing.
//! Explicitly memory-only logins never write a grant. No OS lock spans an await.
//! Lock/keyring failures cause fresh minting without persistence; the minted
//! token is still reused in that process until its fixed deadline. Persistence
//! is currently implemented only on Linux. Clearing does not revoke already
//! issued credentials or discard tokens held by running sessions; revoke on
//! Launchpad for that. Wall-clock rollback across process restarts cannot be
//! detected reliably, so this cache assumes a trustworthy system clock.
//!
//! # Grant storage
//!
//! See [`GrantStore`].  The stored record uses launchpadlib's version-2
//! format, so a file written by either tool can be read by the other.  It
//! holds the refresh token and secret, the account, the optional credential
//! ID, the service root, the WebAuthn origin and the consumer key.  Access
//! tokens are stored only in the separate, optional keyring cache. Challenges,
//! assertions and PINs are never stored, and private keys never leave the key.

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
use zeroize::{Zeroize, Zeroizing};

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

/// Default cross-process cache lifetime (five minutes).
pub const DEFAULT_ACCESS_TOKEN_CACHE_TTL: Duration = Duration::from_secs(300);
/// Maximum permitted cross-process cache lifetime (one day).
pub const MAX_ACCESS_TOKEN_CACHE_TTL: Duration = Duration::from_secs(86400);
const ACCESS_CACHE_USER: &str = "launchpad-hardware-access-token";

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
        self.save_with_cache(credentials, &KeyringAccessTokenCache)
    }

    fn save_with_cache(
        &self,
        credentials: &HardwareTokenCredentials,
        cache: &dyn AccessTokenCache,
    ) -> Result<StoredIn> {
        with_grant_cache_change(self, cache, || self.save_raw(credentials))
    }

    fn save_raw(&self, credentials: &HardwareTokenCredentials) -> Result<StoredIn> {
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

    /// Delete the stored grant and clear its access-token cache. Missing
    /// entries succeed. Contended locks fail before deleting the grant or
    /// rotating its generation; retry after the competing operation finishes.
    /// When no cross-process coordination directory is available (e.g. under
    /// sudo), uncached grant deletion still proceeds. Generation markers fence
    /// outstanding publications when available; a failed fence also aborts
    /// before deletion. Without coordination, persistence is disabled.
    /// Cache deletion is best effort; use
    /// [`clear_cached_access_token`] to require confirmed cache deletion.
    ///
    /// This only removes the local copy; use
    /// [`revoke_hardware_bound_oauth_token`] to revoke it on Launchpad.
    pub fn delete(&self) -> Result<()> {
        self.delete_with_cache(&KeyringAccessTokenCache)
    }

    fn delete_with_cache(&self, cache: &dyn AccessTokenCache) -> Result<()> {
        with_grant_cache_change(self, cache, || self.delete_raw()).map(|_| ())
    }

    fn delete_raw(&self) -> Result<()> {
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
// Keyring-only access-token cache
// ---------------------------------------------------------------------------

trait CacheLock: Send {}
impl CacheLock for std::fs::File {}

trait AccessTokenCache: Send + Sync {
    fn lock(&self) -> Result<Box<dyn CacheLock>>;
    // None means there is no trusted runtime namespace: grant storage can
    // proceed uncached. Contention in a trusted namespace is an error, NEVER
    // an uncached fallback, or it would permit racing a checked login commit.
    fn mutation_lock(&self) -> Result<Option<Box<dyn CacheLock>>> {
        self.lock().map(Some)
    }
    fn read(&self) -> Result<Option<Zeroizing<String>>>;
    fn write(&self, record: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
    fn generation(&self) -> Result<String>;
    // Callers must hold the shared mutation/cache lock while rotating.
    fn rotate_generation(&self) -> Result<String>;
}

struct KeyringAccessTokenCache;

fn with_grant_cache_change<T>(
    store: &GrantStore,
    cache: &dyn AccessTokenCache,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = grant_mutation_lock(store, cache)?;
    if lock.is_none() {
        return operation();
    }
    change_grant_while_locked(cache, operation)
}

fn grant_mutation_lock(
    store: &GrantStore,
    cache: &dyn AccessTokenCache,
) -> Result<Option<Box<dyn CacheLock>>> {
    if *store == GrantStore::Memory || is_sudo() {
        return Ok(None);
    }
    cache.mutation_lock()
}

// The caller holds the mutation lock across both the fence and the grant I/O.
// Cache deletion failures do not prevent grant storage. Lock contention and
// a failed generation fence must fail BEFORE modifying the grant.
fn change_grant_while_locked<T>(
    cache: &dyn AccessTokenCache,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    cache.rotate_generation()?;
    let result = operation();
    // No producer can snapshot during this operation: all use the same lock.
    // The successful pre-mutation rotation fences every earlier import/mint.
    let _ = cache.delete();
    result
}

fn cache_error() -> LpError {
    // Backend errors can contain credential attributes; do not echo them.
    LpError::HardwareToken("The hardware access-token cache is unavailable or busy.".into())
}

fn sudo_cache_disabled_error() -> LpError {
    LpError::HardwareToken(
        "The hardware access-token cache is disabled under sudo; tokens are kept in memory only."
            .into(),
    )
}

#[cfg(target_os = "linux")]
const CACHE_LOCK_WAIT_LIMIT: Duration = Duration::from_millis(100);
#[cfg(target_os = "linux")]
const CACHE_LOCK_RETRY_STEP: Duration = Duration::from_millis(5);

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockFailure {
    Busy,
    Unavailable,
}

impl AccessTokenCache for KeyringAccessTokenCache {
    fn lock(&self) -> Result<Box<dyn CacheLock>> {
        if is_sudo() {
            return Err(sudo_cache_disabled_error());
        }
        // Do not follow a malicious symlink or lock a device/FIFO. The lock
        // contains no credentials and must be owned exclusively by this user.
        #[cfg(target_os = "linux")]
        {
            let directory = trusted_coordination_directory()?;
            lock_directory_with_retry(&directory).map_err(|_| cache_error())
        }
        #[cfg(not(target_os = "linux"))]
        {
            // This Ubuntu client only enables persistence where we can verify
            // the lock's ownership and reject symlinks securely.
            Err(cache_error())
        }
    }

    fn mutation_lock(&self) -> Result<Option<Box<dyn CacheLock>>> {
        #[cfg(target_os = "linux")]
        {
            let Ok(directory) = trusted_coordination_directory() else {
                return Ok(None);
            };
            // Only an unavailable/unsafe coordination directory proceeds uncached.
            // Contention in a checked coordination directory MUST fail cleanly.
            match lock_directory_with_retry(&directory) {
                Ok(lock) => Ok(Some(lock)),
                Err(LockFailure::Unavailable) => Ok(None),
                Err(LockFailure::Busy) => Err(cache_error()),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(None)
        }
    }

    fn read(&self) -> Result<Option<Zeroizing<String>>> {
        let entry = auth::keyring_entry_for(ACCESS_CACHE_USER).map_err(|_| cache_error())?;
        match entry.get_password() {
            Ok(raw) => Ok(Some(Zeroizing::new(raw))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(cache_error()),
        }
    }

    fn write(&self, record: &str) -> Result<()> {
        auth::keyring_entry_for(ACCESS_CACHE_USER)
            .and_then(|entry| entry.set_password(record))
            .map_err(|_| cache_error())
    }

    fn delete(&self) -> Result<()> {
        match auth::keyring_entry_for(ACCESS_CACHE_USER).and_then(|entry| entry.delete_credential())
        {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err(cache_error()),
        }
    }

    fn generation(&self) -> Result<String> {
        #[cfg(target_os = "linux")]
        {
            let directory = trusted_coordination_directory()?;
            let path = directory.join("hardware-access-token.generation");
            match std::fs::read_to_string(&path) {
                Ok(value) if valid_generation(&value) => Ok(value),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.rotate_generation()
                }
                _ => Err(cache_error()),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(cache_error())
        }
    }

    fn rotate_generation(&self) -> Result<String> {
        #[cfg(target_os = "linux")]
        {
            let directory = trusted_coordination_directory()?;
            let value = new_cache_generation();
            // Only a random, non-secret fence is written to disk. Tokens
            // remain keyring-only. Rotate while holding the shared lock.
            write_private_file(
                &directory.join("hardware-access-token.generation"),
                value.as_bytes(),
            )?;
            Ok(value)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(cache_error())
        }
    }
}

fn new_cache_generation() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

fn valid_generation(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_os = "linux")]
fn trusted_coordination_directory() -> Result<PathBuf> {
    if is_sudo() {
        return Err(sudo_cache_disabled_error());
    }
    // SAFETY: geteuid has no memory safety requirements.
    let uid = unsafe { libc::geteuid() };
    let home = passwd_home_directory(uid).ok();
    trusted_coordination_directory_in(home.as_deref(), Path::new("/run/user"), 0, uid)
}

#[cfg(target_os = "linux")]
fn trusted_coordination_directory_in(
    passwd_home: Option<&Path>,
    run_user_root: &Path,
    expected_run_user_parent_uid: u32,
    uid: u32,
) -> Result<PathBuf> {
    if is_sudo() {
        return Err(sudo_cache_disabled_error());
    }
    // Prefer the user's canonical passwd home cache (`~/.cache/lpcli`), independent
    // of HOME/XDG/TMPDIR overrides, so systemd and non-systemd sessions share one
    // namespace without requiring `/run/user/<uid>`. Fall back to `/run/user/<uid>`
    // when home is unavailable or unwritable (e.g. read-only home).
    if let Some(home) = passwd_home
        && let Ok(directory) = trusted_home_cache_directory(home, uid)
    {
        return Ok(directory);
    }
    trusted_runtime_directory_in(run_user_root, expected_run_user_parent_uid, uid)
}

#[cfg(target_os = "linux")]
fn passwd_home_directory(uid: u32) -> Result<PathBuf> {
    use nix::unistd::{Uid, User};

    // nix wraps getpwuid_r and copies its buffer-backed fields into owned values.
    // Unlike home_dir(), this lookup never consults HOME.
    let user = User::from_uid(Uid::from_raw(uid))
        .map_err(|_| cache_error())?
        .ok_or_else(cache_error)?;
    let path = user.dir;
    if path.as_os_str().is_empty() || path == Path::new("/") {
        return Err(cache_error());
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn trusted_home_cache_directory(home: &Path, uid: u32) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if home.as_os_str().is_empty() || home == Path::new("/") {
        return Err(cache_error());
    }
    let home_meta = std::fs::symlink_metadata(home).map_err(|_| cache_error())?;
    if !home_meta.is_dir() || home_meta.uid() != uid || home_meta.mode() & 0o022 != 0 {
        return Err(cache_error());
    }
    let cache_root = home.join(".cache");
    match std::fs::DirBuilder::new().mode(0o700).create(&cache_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(cache_error()),
    }
    let cache_meta = std::fs::symlink_metadata(&cache_root).map_err(|_| cache_error())?;
    if !cache_meta.is_dir() || cache_meta.uid() != uid || cache_meta.mode() & 0o022 != 0 {
        return Err(cache_error());
    }
    let directory = cache_root.join("lpcli");
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(cache_error()),
    }
    validate_runtime_directory(&directory, uid)?;
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn trusted_runtime_directory_in(
    run_user_root: &Path,
    expected_parent_uid: u32,
    uid: u32,
) -> Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    if is_sudo() {
        return Err(sudo_cache_disabled_error());
    }
    // Never create `/run/user` or fall back to `/tmp`.
    let parent = std::fs::symlink_metadata(run_user_root).map_err(|_| cache_error())?;
    if !parent.is_dir() || parent.uid() != expected_parent_uid || parent.mode() & 0o022 != 0 {
        return Err(cache_error());
    }
    let path = run_user_root.join(uid.to_string());
    validate_runtime_directory(&path, uid)?;
    Ok(path)
}

#[cfg(target_os = "linux")]
fn validate_runtime_directory(path: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| cache_error())?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(cache_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn lock_directory_with_retry(
    directory: &Path,
) -> std::result::Result<Box<dyn CacheLock>, LockFailure> {
    let deadline = std::time::Instant::now() + CACHE_LOCK_WAIT_LIMIT;
    loop {
        match secure_cache_lock_checked(directory) {
            Ok(lock) => return Ok(lock),
            Err(LockFailure::Unavailable) => return Err(LockFailure::Unavailable),
            Err(LockFailure::Busy) if std::time::Instant::now() >= deadline => {
                return Err(LockFailure::Busy);
            }
            Err(LockFailure::Busy) => std::thread::sleep(CACHE_LOCK_RETRY_STEP),
        }
    }
}

#[cfg(all(target_os = "linux", test))]
fn secure_cache_lock(directory: &Path) -> Result<Box<dyn CacheLock>> {
    secure_cache_lock_checked(directory).map_err(|_| cache_error())
}

#[cfg(target_os = "linux")]
fn secure_cache_lock_checked(
    directory: &Path,
) -> std::result::Result<Box<dyn CacheLock>, LockFailure> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(LockFailure::Unavailable),
    }
    // SAFETY: geteuid has no arguments or memory safety requirements.
    let uid = unsafe { libc::geteuid() };
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| LockFailure::Unavailable)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(LockFailure::Unavailable);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join("hardware-access-token.lock"))
        .map_err(|_| LockFailure::Unavailable)?;
    let metadata = file.metadata().map_err(|_| LockFailure::Unavailable)?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(LockFailure::Unavailable);
    }
    match file.try_lock() {
        Ok(()) => Ok(Box::new(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockFailure::Busy),
        Err(std::fs::TryLockError::Error(_)) => Err(LockFailure::Unavailable),
    }
}

/// Remove the cross-process hardware access-token cache (not the grant).
/// Missing entries succeed. Busy locks, unavailable keyrings and deletion
/// failures are reported; there is no file fallback. Refuses to run under sudo.
/// This does not revoke tokens on Launchpad or discard in-process tokens;
/// ongoing sessions retain their tokens until their fixed deadlines. Tokens
/// minted before a successful clear cannot become reusable through a late
/// publication: every record must match the current generation. A fresh mint
/// may populate the cache again. Grant deletion also attempts to clear it.
pub fn clear_cached_access_token() -> Result<()> {
    clear_access_cache(&KeyringAccessTokenCache)
}

fn clear_access_cache(cache: &dyn AccessTokenCache) -> Result<()> {
    let _lock = cache.lock()?;
    cache.rotate_generation()?;
    cache.delete()
}

trait CacheClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemCacheClock;
impl CacheClock for SystemCacheClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

fn load_cache_grant(store: &GrantStore) -> Result<Option<HardwareTokenCredentials>> {
    if *store != GrantStore::Keyring {
        return store.load();
    }
    // Unlike GrantStore::load, distinguish backend failure from an absent
    // grant. An unavailable keyring must disable persistence, not falsely
    // report logout and prevent this already-loaded session minting locally.
    match auth::keyring_entry_for(KEYRING_USER).and_then(|entry| entry.get_password()) {
        Ok(raw) => HardwareTokenCredentials::from_json(&Zeroizing::new(raw)).map(Some),
        Err(keyring_core::Error::NoEntry) => load_file(&fallback_path()?),
        Err(_) => Err(cache_error()),
    }
}

struct CachePolicy {
    backend: Arc<dyn AccessTokenCache>,
    ttl: Duration,
    // Checking the live store while holding the lock prevents an old session
    // from repopulating the cache after logout or grant replacement.
    store: GrantStore,
    // Imported grants must authenticate before any cache access is enabled.
    enabled: std::sync::atomic::AtomicBool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessCacheRecord {
    version: u32,
    binding: String,
    token: String,
    secret: String,
    minted_at: DateTime<Utc>,
    server_expires_at: DateTime<Utc>,
    deadline: DateTime<Utc>,
    ttl: Duration,
    generation: String,
}

impl Drop for AccessCacheRecord {
    fn drop(&mut self) {
        self.token.zeroize();
        self.secret.zeroize();
    }
}

fn cache_binding(credentials: &HardwareTokenCredentials) -> String {
    // Length-delimited canonical JSON binds the account, both grant secrets,
    // credential restriction, consumer, service root and WebAuthn origin.
    let raw = credentials
        .to_json()
        .expect("validated hardware credentials");
    hex::encode(Sha256::digest(raw.as_bytes()))
}

fn check_cache_ttl(ttl: Duration) -> Result<()> {
    if ttl > MAX_ACCESS_TOKEN_CACHE_TTL {
        return Err(LpError::HardwareToken(
            "Hardware access-token cache lifetime must be at most 86400 seconds.".into(),
        ));
    }
    Ok(())
}

fn token_deadline(
    minted_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    ttl: Duration,
) -> Option<DateTime<Utc>> {
    let lifetime = expires_at.signed_duration_since(minted_at);
    if lifetime <= TimeDelta::zero() || ttl.is_zero() || check_cache_ttl(ttl).is_err() {
        return None;
    }
    let margin = std::cmp::min(TimeDelta::seconds(60), lifetime / 10);
    Some(std::cmp::min(
        minted_at.checked_add_signed(TimeDelta::from_std(ttl).ok()?)?,
        expires_at.checked_sub_signed(margin)?,
    ))
}

impl AccessCacheRecord {
    fn credentials(
        &self,
        owner: &HardwareTokenCredentials,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> Option<CachedToken> {
        let deadline = token_deadline(self.minted_at, self.server_expires_at, self.ttl)?;
        let requested_deadline = token_deadline(self.minted_at, self.server_expires_at, ttl)?;
        let deadline = std::cmp::min(deadline, requested_deadline);
        if self.version != 2
            || self.binding != cache_binding(owner)
            || !valid_generation(&self.generation)
            || self.token.is_empty()
            || self.secret.is_empty()
            || self.minted_at > now
            || self.deadline != token_deadline(self.minted_at, self.server_expires_at, self.ttl)?
            || now >= deadline
        {
            return None;
        }
        Some(CachedToken {
            credentials: Credentials::new(owner.consumer(), &self.token, &self.secret),
            renew_at: deadline,
            minted_at: self.minted_at,
            expires_at: self.server_expires_at,
        })
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A minted access token and when to replace it.
struct CachedToken {
    credentials: Credentials,
    renew_at: DateTime<Utc>,
    minted_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct SessionCache {
    token: Option<CachedToken>,
    // Fingerprints, not secrets: a failed keyring deletion must not cause this
    // session to reload a rejected generation, but newer tokens remain usable.
    rejected: std::collections::HashSet<String>,
}

fn token_generation(credentials: &Credentials) -> String {
    let raw = Zeroizing::new(
        serde_json::to_string(&(
            &credentials.consumer_key,
            &credentials.access_token.token,
            &credentials.access_token.secret,
        ))
        .expect("OAuth credentials serialize as strings"),
    );
    hex::encode(Sha256::digest(raw.as_bytes()))
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
    cached: tokio::sync::Mutex<SessionCache>,
    cache_policy: Option<CachePolicy>,
    cache_timeout: Duration,
    clock: Arc<dyn CacheClock>,
    #[cfg(test)]
    before_grant_write: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
            cached: tokio::sync::Mutex::new(SessionCache::default()),
            cache_policy: None,
            cache_timeout: Duration::ZERO,
            clock: Arc::new(SystemCacheClock),
            #[cfg(test)]
            before_grant_write: std::sync::Mutex::new(None),
        }
    }

    fn configure_cache(&mut self, store: &GrantStore, timeout: Duration, enabled: bool) {
        self.cache_timeout = timeout;
        if !timeout.is_zero()
            && !is_sudo()
            && *store != GrantStore::Memory
            && cfg!(target_os = "linux")
        {
            self.cache_policy = Some(CachePolicy {
                backend: Arc::new(KeyringAccessTokenCache),
                ttl: timeout,
                store: store.clone(),
                enabled: std::sync::atomic::AtomicBool::new(enabled),
            });
        }
    }

    fn active_cache_policy(&self) -> Option<&CachePolicy> {
        self.cache_policy
            .as_ref()
            .filter(|policy| policy.enabled.load(std::sync::atomic::Ordering::SeqCst))
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
        if let Some(token) = cached.token.as_ref()
            && self.clock.now() >= token.minted_at
            && self.clock.now() < token.renew_at
        {
            // Never refresh a persisted record from this fast path. In-memory
            // reuse survives keyring failures and cannot undo a cache clear.
            return Ok(token.credentials.clone());
        }
        cached.token = None;
        let policy = self.active_cache_policy();
        let mut persistence_lock = policy.and_then(|policy| policy.backend.lock().ok());
        let mut generation = policy.and_then(|policy| {
            persistence_lock.as_ref()?;
            policy.backend.generation().ok()
        });
        if generation.is_none() {
            persistence_lock = None;
        }
        if let (Some(policy), Some(_)) = (policy, &persistence_lock) {
            if policy.store != GrantStore::Memory {
                match load_cache_grant(&policy.store) {
                    Ok(Some(live)) if cache_binding(&live) == cache_binding(&self.credentials) => {}
                    Ok(_) => {
                        return Err(LpError::HardwareToken(
                            "The hardware grant was removed or replaced; load a new session."
                                .into(),
                        ));
                    }
                    Err(_) => persistence_lock = None,
                }
            }
            if persistence_lock.is_some() {
                match policy.backend.read() {
                    Ok(Some(raw)) => {
                        let record = serde_json::from_str::<AccessCacheRecord>(&raw).ok();
                        let token = record
                            .as_ref()
                            .filter(|record| Some(&record.generation) == generation.as_ref())
                            .and_then(|record| {
                                record.credentials(&self.credentials, policy.ttl, self.clock.now())
                            });
                        if let Some(token) = token
                            && !cached
                                .rejected
                                .contains(&token_generation(&token.credentials))
                        {
                            let credentials = token.credentials.clone();
                            cached.token = Some(token);
                            return Ok(credentials);
                        }
                        // Never use an invalid/rejected record, but leave
                        // deletion to explicit or request-aware invalidation.
                        // A fresh mint may replace it under this same lock.
                    }
                    Ok(None) => {}
                    Err(_) => persistence_lock = None,
                }
            }
        }

        // No OS lock spans an await. The generation fences publication after
        // a clear/logout even if they run while minting or verifying.
        if persistence_lock.is_none() {
            generation = None;
        }
        drop(persistence_lock);
        // Start the TTL before the exchange, not at cache write time. Time
        // spent awaiting a touch can only shorten the cache's lifetime.
        let minted_at = self.clock.now();
        let minted = self.mint().await?;
        let remaining = minted.expires_at - self.clock.now();
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
        if cached.rejected.contains(&token_generation(&credentials)) {
            return Err(LpError::HardwareToken(
                "The server returned a previously rejected access token.".into(),
            ));
        }
        let mut renew_at = minted.expires_at - margin;
        if let Some(deadline) = token_deadline(minted_at, minted.expires_at, self.cache_timeout) {
            renew_at = std::cmp::min(renew_at, deadline);
        }
        let token = CachedToken {
            credentials: credentials.clone(),
            renew_at,
            minted_at,
            expires_at: minted.expires_at,
        };
        if let (Some(policy), Some(generation)) = (policy, generation.as_deref())
            && let Ok(_lock) = policy.backend.lock()
        {
            let _ = self.publish_token(policy, &token, generation);
        }
        cached.token = Some(token);
        Ok(credentials)
    }

    fn snapshot_grant_generation(&self, store: &GrantStore) -> Result<Option<String>> {
        let backend: &dyn AccessTokenCache = self
            .cache_policy
            .as_ref()
            .map(|policy| policy.backend.as_ref())
            .unwrap_or(&KeyringAccessTokenCache);
        let lock = grant_mutation_lock(store, backend)?;
        if lock.is_none() {
            return Ok(None);
        }
        backend.generation().map(Some)
    }

    #[cfg(test)]
    fn snapshot_generation(&self) -> Option<String> {
        let policy = self.cache_policy.as_ref()?;
        let _lock = policy.backend.lock().ok()?;
        policy.backend.generation().ok()
    }

    // Caller holds the shared mutation/cache lock through the generation and
    // live-grant checks and the write. Ordinary mints retain their snapshot;
    // a verified replacement uses its new commit epoch. Neither can publish
    // across an intervening clear/logout.
    fn publish_token(
        &self,
        policy: &CachePolicy,
        token: &CachedToken,
        generation: &str,
    ) -> Result<()> {
        if policy.backend.generation()? != generation {
            return Ok(());
        }
        if policy.store != GrantStore::Memory
            && load_cache_grant(&policy.store)?
                .is_none_or(|live| cache_binding(&live) != cache_binding(&self.credentials))
        {
            return Ok(());
        }
        let Some(deadline) = token_deadline(token.minted_at, token.expires_at, policy.ttl) else {
            return Ok(());
        };
        if self.clock.now() < token.minted_at || self.clock.now() >= deadline {
            return Ok(());
        }
        let record = AccessCacheRecord {
            version: 2,
            binding: cache_binding(&self.credentials),
            token: token.credentials.access_token.token.clone(),
            secret: token.credentials.access_token.secret.clone(),
            minted_at: token.minted_at,
            server_expires_at: token.expires_at,
            deadline,
            ttl: policy.ttl,
            generation: generation.to_owned(),
        };
        let raw = Zeroizing::new(serde_json::to_string(&record).map_err(|_| cache_error())?);
        policy.backend.write(&raw)
    }

    // This is called ONLY after current_person has verified the account.
    async fn commit_verified_login(
        &self,
        changed: bool,
        store: &GrantStore,
        generation: Option<&str>,
    ) -> Result<Option<StoredIn>> {
        let cached = self.cached.lock().await;
        let policy = self.cache_policy.as_ref();
        let backend: &dyn AccessTokenCache = policy
            .map(|policy| policy.backend.as_ref())
            .unwrap_or(&KeyringAccessTokenCache);
        // Also required when cache_timeout is zero: an uncached import must
        // not bypass another process's checked grant commit or logout.
        // No OS lock spans an await; this guard covers the entire sync commit.
        let lock = grant_mutation_lock(store, backend)?;
        let current = if lock.is_some() {
            backend.generation().ok()
        } else {
            None
        };
        let unchanged = current
            .as_deref()
            .zip(generation)
            .is_some_and(|(current, generation)| current == generation);
        let persisting_import = changed && *store != GrantStore::Memory;
        let coordination_changed = lock.is_some() != generation.is_some();
        if persisting_import && (coordination_changed || (lock.is_some() && !unchanged)) {
            return Err(LpError::HardwareToken(
                "The cache or grant changed during login; retry the verified import.".into(),
            ));
        }
        #[cfg(test)]
        {
            // Deterministically exercise logout at the check/write boundary.
            let hook = self.before_grant_write.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let (stored, publication_generation) = if persisting_import && lock.is_some() {
            // Consume the checked snapshot before writing: any other import
            // verified against that epoch must now retry, even for the same
            // account. Keep the lock through rotation, grant write and seeding.
            let committed_generation = backend.rotate_generation()?;
            let result = store.save_raw(&self.credentials);
            let _ = backend.delete();
            (Some(result?), Some(committed_generation))
        } else if persisting_import {
            // Coordination is unavailable (e.g. under sudo with an explicit
            // grant file, or read-only home without /run/user/<uid>): save
            // uncached without publishing to the cross-process keyring cache.
            (Some(store.save_raw(&self.credentials)?), None)
        } else if changed {
            // A memory-only login authenticates without committing any grant.
            (Some(StoredIn::Memory), None)
        } else {
            (None, generation.filter(|_| unchanged).map(str::to_owned))
        };
        if let (Some(policy), Some(generation), Some(token)) = (
            policy,
            publication_generation.as_deref(),
            cached.token.as_ref(),
        ) {
            let _ = self.publish_token(policy, token, generation);
        }
        if let Some(policy) = policy {
            policy
                .enabled
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(stored)
    }

    /// Discard this session's current token locally, without deleting any
    /// persisted record. The token will not be reloaded by this session.
    /// This legacy API has no rejected-request identity: a delayed 401 could
    /// discard a newer *local* token. For HTTP rejection handling, use
    /// [`Self::invalidate_rejected_access_token`] with the exact credentials
    /// used to sign the rejected request, not the session's current token.
    pub async fn invalidate(&self) {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.token.take() {
            cached.rejected.insert(token_generation(&token.credentials));
        }
    }

    /// Conditionally invalidate the OAuth token used by a rejected request.
    /// Consumer/key/secret identify its generation; the persisted record must
    /// also match this session's complete grant binding. Newer local tokens
    /// and persisted tokens for other grants/generations are left untouched.
    ///
    /// Returns true if a matching local or persisted token was removed, false
    /// if neither matched. Keyring/lock/deletion failures are reported *after*
    /// local invalidation; this session remembers the rejected generation so
    /// a failed deletion cannot cause it to reload that token. Missing entries
    /// succeed. Other running sessions may still hold the rejected token until
    /// their own request is rejected; this method does not revoke on Launchpad.
    ///
    /// Client integration must retain the credentials used for each signed
    /// request and pass them here on 401, even when a different request has
    /// already renewed the session. Do not call `access_credentials()` merely
    /// to obtain the argument, since that could return a newer generation.
    pub async fn invalidate_rejected_access_token(&self, rejected: &Credentials) -> Result<bool> {
        if rejected.consumer_key != self.credentials.consumer() {
            return Ok(false);
        }
        let mut cached = self.cached.lock().await;
        cached.rejected.insert(token_generation(rejected));
        let removed_local = cached
            .token
            .as_ref()
            .is_some_and(|token| token.credentials == *rejected);
        if removed_local {
            cached.token = None;
        }
        let Some(policy) = self.active_cache_policy() else {
            return Ok(removed_local);
        };
        let _lock = policy.backend.lock()?;
        let Some(raw) = policy.backend.read()? else {
            return Ok(removed_local);
        };
        let matching = serde_json::from_str::<AccessCacheRecord>(&raw)
            .ok()
            .is_some_and(|record| {
                record.binding == cache_binding(&self.credentials)
                    && record.token == rejected.access_token.token
                    && record.secret == rejected.access_token.secret
            });
        if matching {
            policy.backend.delete()?;
        }
        Ok(removed_local || matching)
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
#[derive(Debug, Clone)]
pub struct HardwareLogin {
    grant: Option<HardwareTokenGrant>,
    account: Option<String>,
    consumer: Option<String>,
    credential_id: Option<String>,
    store: GrantStore,
    cache_timeout: Duration,
}

impl Default for HardwareLogin {
    fn default() -> Self {
        Self {
            grant: None,
            account: None,
            consumer: None,
            credential_id: None,
            store: GrantStore::default(),
            cache_timeout: DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        }
    }
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

    /// Set the fixed cache TTL, defaulting to five minutes. Zero disables
    /// persistence, and values above 86400 seconds are rejected by
    /// [`login_with_hardware_token`] before any authentication or storage.
    /// There is no environment-variable override.
    ///
    /// Login always mints fresh and seeds the keyring only AFTER verifying the
    /// account. When cross-process coordination is available, its original
    /// generation must still be current; a clear/logout or another import
    /// during verification forces the persisted import to retry, and successful
    /// replacement rotates to a new epoch and seeds the verified token without
    /// extending its deadline. Cache write failures retain the token locally
    /// until its fixed deadline.
    pub fn cache_timeout(mut self, timeout: Duration) -> Self {
        self.cache_timeout = timeout;
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
/// When a trusted coordination directory is available, persisted imports and
/// replacements require a mutation lock and matching generation snapshot, even
/// with caching disabled, so competing imports, clears or logouts fail safely
/// instead of overwriting or restoring a grant. Under sudo (with an explicit
/// grant file) or when no coordination directory is available, persisted
/// grants still save uncached. Coordination contention or mid-login changes
/// require retrying the login.
///
/// # Errors
///
/// [`LpError::HardwareToken`] when no grant is stored, the grant belongs to
/// another account or consumer, or coordination is contended or changed;
/// [`LpError::HardwareTokenGrantRejected`]
/// when Launchpad refuses the grant; [`LpError::HardwareKey`] and friends
/// when the security key fails.
pub async fn login_with_hardware_token(
    login: HardwareLogin,
    authenticator: Arc<dyn Authenticator>,
) -> Result<HardwareLoginOutcome> {
    check_cache_ttl(login.cache_timeout)?;
    let (credentials, grant_changed) = resolve_credentials(&login)?;

    let account = credentials.grant().account().to_string();
    let mut session = HardwareTokenSession::new(credentials.clone(), authenticator)?;
    session.configure_cache(&login.store, login.cache_timeout, false);
    let generation = session.snapshot_grant_generation(&login.store)?;
    let session = Arc::new(session);
    let client = LaunchpadClient::with_hardware_token(Arc::clone(&session));
    let stored_in = verify_and_store(
        &client,
        &credentials,
        grant_changed,
        &login.store,
        &session,
        generation.as_deref(),
    )
    .await?;

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
    session: &HardwareTokenSession,
    generation: Option<&str>,
) -> Result<Option<StoredIn>> {
    let me = current_person(client).await?;
    if me.name != credentials.grant().account() {
        return Err(LpError::HardwareToken(
            "The grant belongs to a different account.".to_string(),
        ));
    }
    session
        .commit_verified_login(grant_changed, store, generation)
        .await
}

/// Load the stored grant with keyring caching enabled by default (five
/// minutes). Use [`load_session_with_cache`] for an explicit TTL; there is no
/// environment-variable override. Nothing is minted until the first signed
/// request. Under sudo, or for
/// [`GrantStore::Memory`], persistence is always disabled.
pub fn load_session(
    store: &GrantStore,
    authenticator: Arc<dyn Authenticator>,
) -> Result<Option<HardwareTokenSession>> {
    load_session_with_cache(store, authenticator, DEFAULT_ACCESS_TOKEN_CACHE_TTL)
}

/// Load a session with a fixed cross-process cache lifetime. `timeout` is the
/// cache TTL, not the WebAuthn ceremony timeout. Zero disables persistence;
/// values above 86400 seconds are rejected. Shortening the TTL applies to the
/// original mint time, never the time the record is loaded. Tokens are stored
/// only in the keyring, even when the grant itself uses a file store.
/// [`HardwareTokenSession::new`] remains memory-only.
pub fn load_session_with_cache(
    store: &GrantStore,
    authenticator: Arc<dyn Authenticator>,
    timeout: Duration,
) -> Result<Option<HardwareTokenSession>> {
    check_cache_ttl(timeout)?;
    store
        .load()?
        .map(|credentials| {
            let mut session = HardwareTokenSession::new(credentials, authenticator)?;
            session.configure_cache(store, timeout, true);
            Ok(session)
        })
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
/// `name`.  The user must touch the key.  `authenticator` creates the new
/// credential; a hardware-bound `client` may first need its own existing key
/// to authenticate.  Registration does not issue or store a refresh grant.
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
/// This does not remove a stored grant; see [`GrantStore::delete`]. On success
/// it also attempts to clear the local access-token cache.
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
        .await?;
    let _ = clear_cached_access_token();
    Ok(())
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
        registrations: Mutex<Vec<RegistrationRequest>>,
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

        fn make_credential(&self, request: &RegistrationRequest) -> Result<Registration> {
            self.registrations.lock().unwrap().push(request.clone());
            Ok(Registration {
                credential_id: self
                    .credential_id
                    .clone()
                    .unwrap_or_else(|| CREDENTIAL_ID.to_vec()),
                attestation_object: b"fake-attestation".to_vec(),
            })
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

    // -- Cross-process cache -------------------------------------------------

    #[derive(Default)]
    struct FakeAccessCache {
        raw: Mutex<Option<Zeroizing<String>>>,
        locked: Arc<std::sync::atomic::AtomicBool>,
        fail_read: std::sync::atomic::AtomicBool,
        fail_write: std::sync::atomic::AtomicBool,
        fail_delete: std::sync::atomic::AtomicBool,
        writes: AtomicUsize,
        epoch: Mutex<Option<String>>,
        fail_generation: std::sync::atomic::AtomicBool,
        fail_rotation: std::sync::atomic::AtomicBool,
        runtime_unavailable: std::sync::atomic::AtomicBool,
    }

    struct FakeCacheLock(Arc<std::sync::atomic::AtomicBool>);
    impl CacheLock for FakeCacheLock {}
    impl Drop for FakeCacheLock {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    impl AccessTokenCache for FakeAccessCache {
        fn lock(&self) -> Result<Box<dyn CacheLock>> {
            if self.runtime_unavailable.load(Ordering::SeqCst) {
                return Err(cache_error());
            }
            self.locked
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| cache_error())?;
            Ok(Box::new(FakeCacheLock(Arc::clone(&self.locked))))
        }
        fn mutation_lock(&self) -> Result<Option<Box<dyn CacheLock>>> {
            if self.runtime_unavailable.load(Ordering::SeqCst) {
                return Ok(None);
            }
            self.lock().map(Some)
        }
        fn read(&self) -> Result<Option<Zeroizing<String>>> {
            if self.fail_read.load(Ordering::SeqCst) {
                return Err(cache_error());
            }
            Ok(self.raw.lock().unwrap().clone())
        }
        fn write(&self, raw: &str) -> Result<()> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err(cache_error());
            }
            self.writes.fetch_add(1, Ordering::SeqCst);
            *self.raw.lock().unwrap() = Some(Zeroizing::new(raw.to_owned()));
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err(cache_error());
            }
            *self.raw.lock().unwrap() = None;
            Ok(())
        }
        fn generation(&self) -> Result<String> {
            if self.runtime_unavailable.load(Ordering::SeqCst)
                || self.fail_generation.load(Ordering::SeqCst)
            {
                return Err(cache_error());
            }
            Ok(self
                .epoch
                .lock()
                .unwrap()
                .get_or_insert_with(|| "0".repeat(64))
                .clone())
        }
        fn rotate_generation(&self) -> Result<String> {
            if self.fail_rotation.load(Ordering::SeqCst) {
                return Err(cache_error());
            }
            if self.runtime_unavailable.load(Ordering::SeqCst)
                || self.fail_generation.load(Ordering::SeqCst)
            {
                return Err(cache_error());
            }
            let value = new_cache_generation();
            *self.epoch.lock().unwrap() = Some(value.clone());
            Ok(value)
        }
    }

    struct FakeCacheClock(Mutex<DateTime<Utc>>);
    impl FakeCacheClock {
        fn new() -> Self {
            Self(Mutex::new(Utc::now()))
        }
        fn advance(&self, delta: TimeDelta) {
            *self.0.lock().unwrap() += delta;
        }
    }
    impl CacheClock for FakeCacheClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    fn cache_test_record(
        owner: &HardwareTokenCredentials,
        now: DateTime<Utc>,
    ) -> AccessCacheRecord {
        let ttl = DEFAULT_ACCESS_TOKEN_CACHE_TTL;
        let expiry = now + TimeDelta::hours(1);
        AccessCacheRecord {
            version: 2,
            binding: cache_binding(owner),
            token: "cached-access".into(),
            secret: "cached-secret".into(),
            minted_at: now,
            server_expires_at: expiry,
            deadline: token_deadline(now, expiry, ttl).unwrap(),
            ttl,
            generation: "0".repeat(64),
        }
    }

    fn cache_test_session(
        server: &mockito::ServerGuard,
        cache: &Arc<FakeAccessCache>,
        clock: &Arc<FakeCacheClock>,
        authenticator: &Arc<FakeAuthenticator>,
        ttl: Duration,
    ) -> HardwareTokenSession {
        let mut session = session(server, Arc::clone(authenticator), grant());
        session.cache_timeout = ttl;
        if !ttl.is_zero() {
            session.cache_policy = Some(CachePolicy {
                backend: cache.clone(),
                ttl,
                store: GrantStore::Memory,
                enabled: std::sync::atomic::AtomicBool::new(true),
            });
        }
        session.clock = clock.clone();
        session
    }

    async fn cache_mint_mocks(server: &mut mockito::ServerGuard, clock: &Arc<FakeCacheClock>) {
        server
            .mock("POST", "/+hw-challenge")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .create_async()
            .await;
        let clock = Arc::clone(clock);
        let counter = AtomicUsize::new(0);
        server
            .mock("POST", "/+hw-access-token")
            .with_body_from_request(move |_| {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("oauth_token", &format!("access-{n}"))
                    .append_pair("oauth_token_secret", &format!("secret-{n}"))
                    .append_pair(
                        "expires_at",
                        &(clock.now() + TimeDelta::hours(1)).to_rfc3339(),
                    )
                    .finish()
                    .into_bytes()
            })
            .create_async()
            .await;
    }

    #[test]
    fn cache_deadlines_are_fixed_and_conservatively_validated() {
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        let now = Utc::now();
        let mut record = cache_test_record(&owner, now);
        let ttl = DEFAULT_ACCESS_TOKEN_CACHE_TTL;
        assert_eq!(record.deadline, now + TimeDelta::seconds(300));
        assert!(
            record
                .credentials(&owner, ttl, now + TimeDelta::seconds(299))
                .is_some()
        );
        assert!(record.credentials(&owner, ttl, record.deadline).is_none());
        assert!(
            record
                .credentials(&owner, ttl, now - TimeDelta::seconds(1))
                .is_none()
        );
        assert!(record.credentials(&owner, Duration::ZERO, now).is_none());
        assert!(
            record
                .credentials(
                    &owner,
                    Duration::from_secs(100),
                    now + TimeDelta::seconds(100)
                )
                .is_none()
        );
        assert!(
            record
                .credentials(&owner, MAX_ACCESS_TOKEN_CACHE_TTL, record.deadline)
                .is_none()
        );
        assert_eq!(
            token_deadline(now, now + TimeDelta::seconds(100), ttl),
            Some(now + TimeDelta::seconds(90))
        );
        assert_eq!(
            token_deadline(now, now + TimeDelta::hours(1), MAX_ACCESS_TOKEN_CACHE_TTL),
            Some(now + TimeDelta::seconds(3540))
        );
        assert_eq!(token_deadline(now, now, ttl), None);
        record.deadline += TimeDelta::seconds(1);
        assert!(record.credentials(&owner, ttl, now).is_none());
        record.deadline -= TimeDelta::seconds(1);
        record.version = 3;
        assert!(record.credentials(&owner, ttl, now).is_none());
        record.version = 2;
        record.ttl = Duration::from_secs(86401);
        assert!(record.credentials(&owner, ttl, now).is_none());
        assert!(check_cache_ttl(Duration::from_secs(86401)).is_err());
        assert!(check_cache_ttl(MAX_ACCESS_TOKEN_CACHE_TTL).is_ok());
    }

    #[test]
    fn cache_binds_every_grant_context_field() {
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        let now = Utc::now();
        let record = cache_test_record(&owner, now);
        let mut variants = Vec::new();
        for grant in [
            HardwareTokenGrant::new("other-token", "refresh-secret", "example-user").unwrap(),
            HardwareTokenGrant::new("refresh-token", "other-secret", "example-user").unwrap(),
            HardwareTokenGrant::new("refresh-token", "refresh-secret", "other-account").unwrap(),
            grant().with_credential_id("AAAA").unwrap(),
        ] {
            variants.push(HardwareTokenCredentials::new(grant, HARDWARE_CONSUMER_KEY).unwrap());
        }
        let mut changed = owner.clone();
        changed.consumer = "other-consumer".into();
        variants.push(changed);
        let mut changed = owner.clone();
        changed.origin = "https://staging.launchpad.net".into();
        variants.push(changed);
        let mut changed = owner.clone();
        changed.service_root = "https://api.staging.launchpad.net/".into();
        variants.push(changed);
        for changed in variants {
            assert!(
                record
                    .credentials(&changed, DEFAULT_ACCESS_TOKEN_CACHE_TTL, now)
                    .is_none()
            );
        }
        let raw = serde_json::to_string(&record).unwrap();
        for forbidden in [
            "refresh-token",
            "refresh-secret",
            "example-user",
            "assertion",
            "PIN",
        ] {
            assert!(!raw.contains(forbidden));
        }
    }

    #[tokio::test]
    async fn cache_reuses_without_sliding_and_clear_preserves_in_process_tokens() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let ttl = DEFAULT_ACCESS_TOKEN_CACHE_TTL;
        let first = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        assert_eq!(
            first.access_credentials().await.unwrap().access_token.token,
            "access-1"
        );
        let raw = cache.read().unwrap().unwrap();
        clock.advance(TimeDelta::seconds(299));
        let second = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        assert_eq!(
            second
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-1"
        );
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
        clock.advance(TimeDelta::seconds(1));
        assert_eq!(
            second
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-2"
        );
        clear_access_cache(cache.as_ref()).unwrap();
        let retained = second.access_credentials().await.unwrap();
        assert_eq!(retained.access_token.token, "access-2");
        assert!(cache.read().unwrap().is_none());
        let third = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        let fresh = third.access_credentials().await.unwrap();
        assert_eq!(fresh.access_token.token, "access-3");
        assert!(
            third
                .invalidate_rejected_access_token(&fresh)
                .await
                .unwrap()
        );
        assert!(cache.read().unwrap().is_none());
        assert_eq!(
            third.access_credentials().await.unwrap().access_token.token,
            "access-4"
        );
        assert_eq!(authenticator.requests.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn cache_shorter_configuration_uses_original_mint_time_and_zero_is_memory_only() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let first = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        first.access_credentials().await.unwrap();
        clock.advance(TimeDelta::seconds(100));
        let shorter = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            Duration::from_secs(100),
        );
        assert_eq!(
            shorter
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-2"
        );
        let raw = cache.read().unwrap().unwrap();
        let disabled = cache_test_session(&server, &cache, &clock, &authenticator, Duration::ZERO);
        assert_eq!(
            disabled
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-3"
        );
        assert_eq!(
            disabled
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-3"
        );
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
    }

    #[tokio::test]
    async fn cache_backend_failures_and_malformed_records_mint_fresh() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        for raw in ["{SECRET", r#"{"version":1,"token":"SECRET"}"#] {
            cache.write(raw).unwrap();
            let fresh_session = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            fresh_session.access_credentials().await.unwrap();
        }
        cache.fail_read.store(true, Ordering::SeqCst);
        let writes = cache.writes.load(Ordering::SeqCst);
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-3"
        );
        assert_eq!(cache.writes.load(Ordering::SeqCst), writes);
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-3"
        );
        cache.fail_read.store(false, Ordering::SeqCst);
        cache.delete().unwrap();
        cache.fail_write.store(true, Ordering::SeqCst);
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-4"
        );
        assert!(cache.read().unwrap().is_none());
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-4"
        );
        cache.fail_delete.store(true, Ordering::SeqCst);
        assert!(clear_access_cache(cache.as_ref()).is_err());
        cache.fail_delete.store(false, Ordering::SeqCst);
        let lock = cache.lock().unwrap();
        assert!(clear_access_cache(cache.as_ref()).is_err());
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-5"
        );
        assert!(cache.read().unwrap().is_none());
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-5"
        );
        clock.advance(TimeDelta::seconds(300));
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-6"
        );
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "access-6"
        );
        assert_eq!(authenticator.requests.lock().unwrap().len(), 6);
        drop(lock);
        clear_access_cache(cache.as_ref()).unwrap();
        clear_access_cache(cache.as_ref()).unwrap();
    }

    #[tokio::test]
    async fn rejected_cached_token_is_not_reloaded_when_deletion_fails() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        cache
            .write(&serde_json::to_string(&cache_test_record(&owner, clock.now())).unwrap())
            .unwrap();
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        assert_eq!(
            session
                .access_credentials()
                .await
                .unwrap()
                .access_token
                .token,
            "cached-access"
        );
        let rejected = session.access_credentials().await.unwrap();
        cache.fail_delete.store(true, Ordering::SeqCst);
        assert!(
            session
                .invalidate_rejected_access_token(&rejected)
                .await
                .is_err()
        );
        let challenge = server
            .mock("POST", "/+hw-challenge")
            .with_status(410)
            .expect(2)
            .create_async()
            .await;
        for _ in 0..2 {
            assert!(matches!(
                session.access_credentials().await,
                Err(LpError::HardwareTokenGrantRejected)
            ));
        }
        challenge.assert_async().await;
        assert!(authenticator.requests.lock().unwrap().is_empty());
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replaced_or_deleted_live_grant_cannot_repopulate_cache() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grant.json");
        let mut session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        write_private_file(&path, session.credentials.to_json().unwrap().as_bytes()).unwrap();
        session.cache_policy.as_mut().unwrap().store = GrantStore::File(path.clone());
        session.access_credentials().await.unwrap();
        let replacement = HardwareTokenCredentials::new(
            HardwareTokenGrant::new("replacement", "replacement-secret", "example-user").unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        write_private_file(&path, replacement.to_json().unwrap().as_bytes()).unwrap();
        // Local tokens survive grant deletion/replacement too. They cannot
        // repopulate the cache; after their deadline the live store is checked.
        assert!(session.access_credentials().await.is_ok());
        clock.advance(TimeDelta::seconds(300));
        assert!(session.access_credentials().await.is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(session.access_credentials().await.is_err());
        assert_eq!(authenticator.requests.lock().unwrap().len(), 1);
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_future_and_wrong_binding_records_never_reuse() {
        let mut server = mockito::Server::new_async().await;
        let clock = Arc::new(FakeCacheClock::new());
        let cache = Arc::new(FakeAccessCache::default());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        let mut expired =
            cache_test_record(&session.credentials, clock.now() - TimeDelta::seconds(300));
        let future = cache_test_record(&session.credentials, clock.now() + TimeDelta::seconds(1));
        let mut wrong_binding = cache_test_record(&session.credentials, clock.now());
        wrong_binding.binding = "wrong-grant".into();
        // Empty credentials are invalid even if all timestamps/binding match.
        let mut empty = cache_test_record(&session.credentials, clock.now());
        empty.secret.clear();
        for record in [&mut expired, &mut wrong_binding, &mut empty] {
            cache
                .write(&serde_json::to_string(record).unwrap())
                .unwrap();
            let session = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            assert_ne!(
                session
                    .access_credentials()
                    .await
                    .unwrap()
                    .access_token
                    .token,
                "cached-access"
            );
        }
        cache
            .write(&serde_json::to_string(&future).unwrap())
            .unwrap();
        session.access_credentials().await.unwrap();
        assert_eq!(authenticator.requests.lock().unwrap().len(), 4);
    }

    #[test]
    fn cache_policy_child() {
        let Some(path) = std::env::var_os("LPCLI_TEST_CACHE_POLICY_GRANT") else {
            return;
        };
        let store = GrantStore::File(PathBuf::from(path));
        let authenticator: Arc<dyn Authenticator> = Arc::new(FakeAuthenticator::default());
        let expected = std::env::var("LPCLI_TEST_CACHE_POLICY_EXPECT").unwrap();
        let loaded = match std::env::var("LPCLI_TEST_CACHE_TIMEOUT") {
            Ok(value) => load_session_with_cache(
                &store,
                Arc::clone(&authenticator),
                Duration::from_secs(value.parse().unwrap()),
            ),
            Err(_) => load_session(&store, Arc::clone(&authenticator)),
        };
        if expected == "invalid" {
            assert!(loaded.is_err());
            return;
        }
        let session = loaded.unwrap().unwrap();
        if expected == "disabled" {
            assert!(session.cache_policy.is_none());
        } else {
            assert_eq!(
                session.cache_policy.as_ref().unwrap().ttl.as_secs(),
                expected.parse::<u64>().unwrap()
            );
        }
        let disabled = load_session_with_cache(&store, Arc::clone(&authenticator), Duration::ZERO)
            .unwrap()
            .unwrap();
        assert!(disabled.cache_policy.is_none());
        assert!(
            load_session_with_cache(
                &store,
                Arc::clone(&authenticator),
                Duration::from_secs(86401)
            )
            .is_err()
        );
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        assert!(
            HardwareTokenSession::new(owner, authenticator)
                .unwrap()
                .cache_policy
                .is_none()
        );
        if is_sudo() {
            assert!(clear_cached_access_token().is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_configurable_cache_policy_and_sudo_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grant.json");
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        write_private_file(&path, owner.to_json().unwrap().as_bytes()).unwrap();
        // Child processes avoid mutating the environment of parallel tests.
        for (value, sudo, expected) in [
            (None, false, "300"),
            (Some("0"), false, "disabled"),
            (Some("42"), false, "42"),
            (Some("86400"), false, "86400"),
            (Some("86401"), false, "invalid"),
            (None, true, "disabled"),
            (Some("86400"), true, "disabled"),
        ] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args(["--exact", "hardware_tokens::tests::cache_policy_child"])
                .env("LPCLI_TEST_CACHE_POLICY_GRANT", &path)
                .env("LPCLI_TEST_CACHE_POLICY_EXPECT", expected)
                .env_remove("SUDO_USER")
                .env_remove("SUDO_UID")
                .env_remove("SUDO_GID")
                // A former TTL environment variable must never override the
                // explicit Duration or the library's default.
                .env("LP_HARDWARE_TOKEN_CACHE_TTL", "invalid-unused-value")
                .env_remove("LPCLI_TEST_CACHE_TIMEOUT");
            if let Some(value) = value {
                child.env("LPCLI_TEST_CACHE_TIMEOUT", value);
            }
            if sudo {
                child.env("SUDO_UID", "1000");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[tokio::test]
    async fn login_cache_timeout_defaults_and_validation() {
        assert_eq!(
            HardwareLogin::default().cache_timeout,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL
        );
        assert_eq!(
            HardwareLogin::new().cache_timeout,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL
        );
        let authenticator = Arc::new(FakeAuthenticator::default());
        let invalid = HardwareLogin::new()
            .store(GrantStore::Memory)
            .cache_timeout(Duration::from_secs(86401));
        let error = login_with_hardware_token(invalid, authenticator.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("86400"));
        assert!(authenticator.requests.lock().unwrap().is_empty());
        for ttl in [
            Duration::ZERO,
            Duration::from_secs(42),
            MAX_ACCESS_TOKEN_CACHE_TTL,
        ] {
            let login = HardwareLogin::default().cache_timeout(ttl);
            assert_eq!(login.cache_timeout, ttl);
            assert!(check_cache_ttl(login.cache_timeout).is_ok());
        }
    }

    #[tokio::test]
    async fn pre_clear_login_token_is_not_published_but_later_mints_can_persist() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let ttl = Duration::from_secs(42);
        let login = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        let policy = login.cache_policy.as_ref().unwrap();
        policy.enabled.store(false, Ordering::SeqCst);
        let generation = login.snapshot_generation().unwrap();
        cache
            .write(
                &serde_json::to_string(&cache_test_record(&login.credentials, clock.now()))
                    .unwrap(),
            )
            .unwrap();
        // Login must mint fresh without reading, modifying, or publishing to
        // the cache before verification. Model clear during account verification.
        assert_eq!(
            login.access_credentials().await.unwrap().access_token.token,
            "access-1"
        );
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
        clear_access_cache(cache.as_ref()).unwrap();
        login
            .commit_verified_login(false, &GrantStore::Memory, Some(&generation))
            .await
            .unwrap();
        assert_eq!(
            login.access_credentials().await.unwrap().access_token.token,
            "access-1"
        );
        assert!(cache.read().unwrap().is_none());
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
        // The cleared login token is not visible to another invocation.
        let other = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        assert_eq!(
            other.access_credentials().await.unwrap().access_token.token,
            "access-2"
        );
        clear_access_cache(cache.as_ref()).unwrap();
        clock.advance(TimeDelta::seconds(42));
        assert_eq!(
            login.access_credentials().await.unwrap().access_token.token,
            "access-3"
        );
        assert!(cache.read().unwrap().is_some());
    }

    #[tokio::test]
    async fn verified_import_seeds_cache_and_failed_verification_preserves_grant() {
        for verified in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let cache = Arc::new(FakeAccessCache::default());
            let clock = Arc::new(FakeCacheClock::new());
            let authenticator = Arc::new(FakeAuthenticator::default());
            cache_mint_mocks(&mut server, &clock).await;
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("grant.json");
            let store = GrantStore::File(path.clone());
            let previous = HardwareTokenCredentials::new(
                HardwareTokenGrant::new("previous", "previous-secret", "example-user").unwrap(),
                HARDWARE_CONSUMER_KEY,
            )
            .unwrap();
            write_private_file(&path, previous.to_json().unwrap().as_bytes()).unwrap();
            let mut session = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            let policy = session.cache_policy.as_mut().unwrap();
            policy.store = store.clone();
            policy.enabled.store(false, Ordering::SeqCst);
            let generation = session.snapshot_generation().unwrap();
            let session = Arc::new(session);
            server
                .mock("GET", Matcher::Regex("^/devel/?$".into()))
                .with_body(json!({"me_link": format!("{}/me", server.url())}).to_string())
                .create_async()
                .await;
            server.mock("GET", "/me")
                .with_body(json!({"name": if verified { "example-user" } else { "wrong-account" }, "self_link": format!("{}/me", server.url())}).to_string())
                .create_async().await;
            let client = LaunchpadClient::with_hardware_token(session.clone())
                .with_base_url(format!("{}/devel", server.url()));
            let result = verify_and_store(
                &client,
                &session.credentials,
                true,
                &store,
                &session,
                Some(&generation),
            )
            .await;
            if verified {
                assert!(result.is_ok());
                assert_eq!(store.load().unwrap().unwrap(), session.credentials);
                let committed_generation = cache.generation().unwrap();
                assert_ne!(committed_generation, generation);
                let record: AccessCacheRecord =
                    serde_json::from_str(&cache.read().unwrap().unwrap()).unwrap();
                assert_eq!(record.generation, committed_generation);
                let mut next = cache_test_session(
                    &server,
                    &cache,
                    &clock,
                    &authenticator,
                    DEFAULT_ACCESS_TOKEN_CACHE_TTL,
                );
                next.cache_policy.as_mut().unwrap().store = store.clone();
                assert_eq!(
                    next.access_credentials().await.unwrap().access_token.token,
                    "access-1"
                );
                assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
            } else {
                assert!(result.is_err());
                assert_eq!(cache.generation().unwrap(), generation);
                assert_eq!(store.load().unwrap().unwrap(), previous);
                assert!(cache.read().unwrap().is_none());
            }
            assert_eq!(authenticator.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn concurrent_verified_imports_consume_snapshot_and_seed_commit_epoch() {
        for first_wins in [true, false] {
            let mut server = mockito::Server::new_async().await;
            let cache = Arc::new(FakeAccessCache::default());
            let clock = Arc::new(FakeCacheClock::new());
            let authenticator = Arc::new(FakeAuthenticator::default());
            cache_mint_mocks(&mut server, &clock).await;
            let directory = tempfile::tempdir().unwrap();
            let store = GrantStore::File(directory.path().join("grant.json"));
            let previous = HardwareTokenCredentials::new(
                HardwareTokenGrant::new("previous", "previous-secret", "example-user").unwrap(),
                HARDWARE_CONSUMER_KEY,
            )
            .unwrap();
            store.save_raw(&previous).unwrap();
            let mut first = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            let mut second = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            second.credentials = HardwareTokenCredentials::new(
                HardwareTokenGrant::new("second-refresh", "second-refresh-secret", "example-user")
                    .unwrap(),
                HARDWARE_CONSUMER_KEY,
            )
            .unwrap();
            for session in [&mut first, &mut second] {
                let policy = session.cache_policy.as_mut().unwrap();
                policy.store = store.clone();
                policy.enabled.store(false, Ordering::SeqCst);
            }
            let original = first.snapshot_grant_generation(&store).unwrap().unwrap();
            assert_eq!(
                second.snapshot_grant_generation(&store).unwrap().unwrap(),
                original
            );
            let first = Arc::new(first);
            let second = Arc::new(second);
            let root = server
                .mock("GET", Matcher::Regex("^/devel/?$".into()))
                .with_body(json!({"me_link": format!("{}/me", server.url())}).to_string())
                .expect(2)
                .create_async()
                .await;
            let me = server
                .mock("GET", "/me")
                .with_body(
                    json!({"name": "example-user", "self_link": format!("{}/me", server.url())})
                        .to_string(),
                )
                .expect(2)
                .create_async()
                .await;
            let minted_at = clock.now();
            // Both imports authenticate before either commits, and both hold
            // the same pre-verification epoch. No scheduling or sleeps needed.
            for session in [&first, &second] {
                let client = LaunchpadClient::with_hardware_token(session.clone())
                    .with_base_url(format!("{}/devel", server.url()));
                assert_eq!(current_person(&client).await.unwrap().name, "example-user");
            }
            root.assert_async().await;
            me.assert_async().await;
            assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
            clock.advance(TimeDelta::seconds(100));
            let (winner, loser, expected_access) = if first_wins {
                (&first, &second, "access-1")
            } else {
                (&second, &first, "access-2")
            };
            winner
                .commit_verified_login(true, &store, Some(&original))
                .await
                .unwrap();
            let committed = cache.generation().unwrap();
            assert_ne!(committed, original);
            let raw = cache.read().unwrap().unwrap();
            let record: AccessCacheRecord = serde_json::from_str(&raw).unwrap();
            assert_eq!(record.generation, committed);
            assert_eq!(record.binding, cache_binding(&winner.credentials));
            assert_eq!(record.token, expected_access);
            assert_eq!(record.minted_at, minted_at);
            assert_eq!(record.deadline, minted_at + TimeDelta::seconds(300));
            assert!(
                loser
                    .commit_verified_login(true, &store, Some(&original))
                    .await
                    .is_err()
            );
            assert_eq!(store.load().unwrap().unwrap(), winner.credentials);
            assert_eq!(cache.generation().unwrap(), committed);
            assert_eq!(*cache.read().unwrap().unwrap(), *raw);
            assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
            // The very next invocation can reuse the winner's verified token.
            let mut next = cache_test_session(
                &server,
                &cache,
                &clock,
                &authenticator,
                DEFAULT_ACCESS_TOKEN_CACHE_TTL,
            );
            next.credentials = winner.credentials.clone();
            next.cache_policy.as_mut().unwrap().store = store.clone();
            assert_eq!(
                next.access_credentials().await.unwrap().access_token.token,
                expected_access
            );
            assert_eq!(authenticator.requests.lock().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn uncached_import_succeeds_without_coordination_and_publishes_no_cache() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        cache.runtime_unavailable.store(true, Ordering::SeqCst);
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let directory = tempfile::tempdir().unwrap();
        let store = GrantStore::File(directory.path().join("grant.json"));
        let mut login = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        login.cache_policy.as_mut().unwrap().store = store.clone();
        login
            .cache_policy
            .as_ref()
            .unwrap()
            .enabled
            .store(false, Ordering::SeqCst);
        assert!(login.snapshot_grant_generation(&store).unwrap().is_none());
        let token = login.access_credentials().await.unwrap();
        // When no coordination directory is available (e.g. sudo with an explicit
        // grant file or read-only home without /run/user), verified persisted
        // imports still save the grant uncached and publish nothing to keyring.
        assert_eq!(
            login
                .commit_verified_login(true, &store, None)
                .await
                .unwrap(),
            Some(StoredIn::File(directory.path().join("grant.json")))
        );
        assert_eq!(store.load().unwrap().as_ref(), Some(&login.credentials));
        assert!(cache.read().unwrap().is_none());
        assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
        // If coordination becomes available mid-login, an unfenced snapshot is
        // rejected rather than bypassing concurrent fenced commits/logouts.
        cache.runtime_unavailable.store(false, Ordering::SeqCst);
        assert!(
            login
                .commit_verified_login(true, &store, None)
                .await
                .is_err()
        );
        cache.runtime_unavailable.store(true, Ordering::SeqCst);
        // Non-mutating and memory-only logins also keep working uncached.
        with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).unwrap();
        assert!(
            login
                .commit_verified_login(false, &store, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(login.access_credentials().await.unwrap(), token);
        assert_eq!(
            login
                .commit_verified_login(true, &GrantStore::Memory, None)
                .await
                .unwrap(),
            Some(StoredIn::Memory)
        );
        assert!(store.load().unwrap().is_none());
        assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
        assert_eq!(authenticator.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn verified_import_rotation_failure_preserves_grant_and_cache() {
        let server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        let directory = tempfile::tempdir().unwrap();
        let store = GrantStore::File(directory.path().join("grant.json"));
        let previous = HardwareTokenCredentials::new(
            HardwareTokenGrant::new("previous", "previous-secret", "example-user").unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        store.save_raw(&previous).unwrap();
        let mut login = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        login.cache_policy.as_mut().unwrap().store = store.clone();
        cache
            .write(&serde_json::to_string(&cache_test_record(&previous, clock.now())).unwrap())
            .unwrap();
        let original = login.snapshot_grant_generation(&store).unwrap().unwrap();
        let raw = cache.read().unwrap().unwrap();
        cache.fail_rotation.store(true, Ordering::SeqCst);
        assert!(
            login
                .commit_verified_login(true, &store, Some(&original))
                .await
                .is_err()
        );
        assert_eq!(store.load().unwrap().unwrap(), previous);
        assert_eq!(cache.generation().unwrap(), original);
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
        assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pre_logout_verified_import_cannot_restore_grant_or_seed_cache() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let directory = tempfile::tempdir().unwrap();
        let store = GrantStore::File(directory.path().join("grant.json"));
        let mut login = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        let policy = login.cache_policy.as_mut().unwrap();
        policy.store = store.clone();
        policy.enabled.store(false, Ordering::SeqCst);
        store.save_raw(&login.credentials).unwrap();
        let generation = login.snapshot_generation().unwrap();
        login.access_credentials().await.unwrap();
        // Model logout while current_person is verifying the new import.
        with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).unwrap();
        assert!(
            login
                .commit_verified_login(true, &store, Some(&generation))
                .await
                .is_err()
        );
        assert!(store.load().unwrap().is_none());
        assert!(cache.read().unwrap().is_none());
        assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn logout_after_verified_import_checks_cannot_resurrect_grant() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let directory = tempfile::tempdir().unwrap();
        let store = GrantStore::File(directory.path().join("grant.json"));
        let previous = HardwareTokenCredentials::new(
            HardwareTokenGrant::new("previous", "previous-secret", "example-user").unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        store.save_raw(&previous).unwrap();
        let mut login = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        let policy = login.cache_policy.as_mut().unwrap();
        policy.store = store.clone();
        policy.enabled.store(false, Ordering::SeqCst);
        let generation = login.snapshot_generation().unwrap();
        login.access_credentials().await.unwrap();
        let attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let attempted_hook = attempted.clone();
        let cache_hook = cache.clone();
        let store_hook = store.clone();
        let previous_hook = previous.clone();
        let generation_hook = generation.clone();
        *login.before_grant_write.lock().unwrap() = Some(Box::new(move || {
            // Exactly after login's generation checks, before save_raw: logout
            // and replacement must fail before either grant or epoch changes.
            assert!(
                with_grant_cache_change(&store_hook, cache_hook.as_ref(), || store_hook
                    .delete_raw())
                .is_err()
            );
            assert!(
                with_grant_cache_change(&store_hook, cache_hook.as_ref(), || store_hook
                    .save_raw(&previous_hook))
                .is_err()
            );
            assert_eq!(store_hook.load().unwrap().unwrap(), previous_hook);
            assert_eq!(cache_hook.generation().unwrap(), generation_hook);
            attempted_hook.store(true, Ordering::SeqCst);
        }));
        login
            .commit_verified_login(true, &store, Some(&generation))
            .await
            .unwrap();
        assert!(attempted.load(Ordering::SeqCst));
        assert_eq!(store.load().unwrap().unwrap(), login.credentials);
        assert!(cache.read().unwrap().is_some());
        // Retry logout after the commit releases the lock. It now succeeds,
        // and replaying the pre-logout verified import cannot restore the grant.
        with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(cache.read().unwrap().is_none());
        assert!(
            login
                .commit_verified_login(true, &store, Some(&generation))
                .await
                .is_err()
        );
        assert!(store.load().unwrap().is_none());
        assert!(cache.read().unwrap().is_none());
    }

    #[tokio::test]
    async fn contended_verified_import_never_falls_back_to_unlocked_write() {
        let server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        let directory = tempfile::tempdir().unwrap();
        let store = GrantStore::File(directory.path().join("grant.json"));
        let previous = HardwareTokenCredentials::new(
            HardwareTokenGrant::new("previous", "previous-secret", "example-user").unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        store.save_raw(&previous).unwrap();
        let mut login = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        login.cache_policy.as_mut().unwrap().store = store.clone();
        let generation = login.snapshot_generation().unwrap();
        let lock = cache.lock().unwrap();
        assert!(login.snapshot_grant_generation(&store).is_err());
        for snapshot in [None, Some(generation.as_str())] {
            assert!(
                login
                    .commit_verified_login(true, &store, snapshot)
                    .await
                    .is_err()
            );
            assert_eq!(store.load().unwrap().unwrap(), previous);
            assert_eq!(cache.generation().unwrap(), generation);
            assert!(cache.read().unwrap().is_none());
        }
        drop(lock);
        // An import without a protected snapshot must not be promoted into a
        // trusted mutation context after verification.
        assert!(
            login
                .commit_verified_login(true, &store, None)
                .await
                .is_err()
        );
        assert_eq!(store.load().unwrap().unwrap(), previous);
        // A failed fence cannot report successful logout while leaving the
        // pre-logout generation valid for a later verified import.
        cache.fail_generation.store(true, Ordering::SeqCst);
        assert!(login.snapshot_grant_generation(&store).is_err());
        assert!(with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).is_err());
        assert_eq!(store.load().unwrap().unwrap(), previous);
        cache.fail_generation.store(false, Ordering::SeqCst);
        // Losing coordination mid-login must abort rather than silently saving
        // when the snapshot expected fencing. Existing-grant sessions still work.
        cache.runtime_unavailable.store(true, Ordering::SeqCst);
        assert!(
            login
                .commit_verified_login(true, &store, Some(&generation))
                .await
                .is_err()
        );
        assert_eq!(store.load().unwrap().unwrap(), previous);
        assert!(
            login
                .commit_verified_login(false, &store, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.load().unwrap().unwrap(), previous);
        assert!(cache.read().unwrap().is_none());
        with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[tokio::test]
    async fn obsolete_publication_after_rotation_is_never_reused() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        let obsolete = cache_test_record(&owner, clock.now());
        clear_access_cache(cache.as_ref()).unwrap();
        // Model a rotation between the producer's last comparison and its
        // keyring write. The stale bytes are harmless because reads also fence.
        cache
            .write(&serde_json::to_string(&obsolete).unwrap())
            .unwrap();
        let next = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        assert_eq!(
            next.access_credentials().await.unwrap().access_token.token,
            "access-1"
        );
        assert_eq!(authenticator.requests.lock().unwrap().len(), 1);
    }

    struct PausingAuthenticator {
        ready: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        delegate: FakeAuthenticator,
    }
    impl Authenticator for PausingAuthenticator {
        fn get_assertion(&self, request: &AssertionRequest) -> Result<Assertion> {
            if let Some(ready) = self.ready.lock().unwrap().take() {
                let _ = ready.send(());
            }
            self.release.lock().unwrap().recv().unwrap();
            self.delegate.get_assertion(request)
        }
        fn make_credential(
            &self,
            request: &RegistrationRequest,
        ) -> Result<crate::hardware_key::Registration> {
            self.delegate.make_credential(request)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn logout_during_touch_fences_producer_without_blocking_runtime() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let (ready_send, ready) = tokio::sync::oneshot::channel();
        let (release, release_receive) = std::sync::mpsc::channel();
        let mut session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grant.json");
        write_private_file(&path, session.credentials.to_json().unwrap().as_bytes()).unwrap();
        let store = GrantStore::File(path.clone());
        session.cache_policy.as_mut().unwrap().store = store.clone();
        session.authenticator = Arc::new(PausingAuthenticator {
            ready: Mutex::new(Some(ready_send)),
            release: Mutex::new(release_receive),
            delegate: FakeAuthenticator::default(),
        });
        let session = Arc::new(session);
        let producer = session.clone();
        let pending = tokio::spawn(async move { producer.access_credentials().await });
        ready.await.unwrap();
        // A touch does not hold the OS cache lock. Also deliberately model a
        // busy lock owned by another process during synchronous grant deletion.
        let lock = cache.lock().unwrap();
        let start = std::time::Instant::now();
        let generation = cache.generation().unwrap();
        assert!(with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(path.exists());
        assert_eq!(cache.generation().unwrap(), generation);
        drop(lock);
        with_grant_cache_change(&store, cache.as_ref(), || store.delete_raw()).unwrap();
        assert!(!path.exists());
        release.send(()).unwrap();
        let credentials = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(credentials.access_token.token, "access-1");
        assert!(cache.read().unwrap().is_none());
        assert_eq!(session.access_credentials().await.unwrap(), credentials);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unsafe_or_missing_runtime_does_not_break_grant_changes() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        // SAFETY: geteuid has no memory safety requirements.
        let uid = unsafe { libc::geteuid() };
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_runtime_directory(&runtime, uid).is_ok());
        assert!(validate_runtime_directory(&runtime, uid.wrapping_add(1)).is_err());
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_runtime_directory(&runtime, uid).is_err());
        std::fs::remove_dir(&runtime).unwrap();
        assert!(validate_runtime_directory(&runtime, uid).is_err());
        symlink(directory.path(), &runtime).unwrap();
        assert!(validate_runtime_directory(&runtime, uid).is_err());
        let cache = FakeAccessCache::default();
        cache.runtime_unavailable.store(true, Ordering::SeqCst);
        let store = GrantStore::File(directory.path().join("grant.json"));
        let owner = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        with_grant_cache_change(&store, &cache, || store.save_raw(&owner)).unwrap();
        assert_eq!(store.load().unwrap().unwrap(), owner);
        with_grant_cache_change(&store, &cache, || store.delete_raw()).unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(clear_access_cache(&cache).is_err());
    }

    #[tokio::test]
    async fn delayed_rejection_preserves_newer_local_and_persisted_generations() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let ttl = DEFAULT_ACCESS_TOKEN_CACHE_TTL;
        let session = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        let rejected = session.access_credentials().await.unwrap();
        clock.advance(TimeDelta::seconds(300));
        let newer = session.access_credentials().await.unwrap();
        let raw = cache.read().unwrap().unwrap();
        assert!(
            !session
                .invalidate_rejected_access_token(&rejected)
                .await
                .unwrap()
        );
        assert_eq!(session.access_credentials().await.unwrap(), newer);
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
        // Another process publishes a newer token while this process still
        // holds its older token. Only this process's matching token is removed.
        let other = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        other.access_credentials().await.unwrap();
        clear_access_cache(cache.as_ref()).unwrap();
        let third = cache_test_session(&server, &cache, &clock, &authenticator, ttl);
        let newest = third.access_credentials().await.unwrap();
        let raw = cache.read().unwrap().unwrap();
        assert!(
            other
                .invalidate_rejected_access_token(&newer)
                .await
                .unwrap()
        );
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
        assert_eq!(other.access_credentials().await.unwrap(), newest);
        // Same token identity in a record bound to another grant is untouched.
        let mut different_grant = cache_test_record(&session.credentials, clock.now());
        different_grant.binding = "different-grant".into();
        different_grant.token = newest.access_token.token.clone();
        different_grant.secret = newest.access_token.secret.clone();
        cache
            .write(&serde_json::to_string(&different_grant).unwrap())
            .unwrap();
        let raw = cache.read().unwrap().unwrap();
        assert!(
            third
                .invalidate_rejected_access_token(&newest)
                .await
                .unwrap()
        );
        assert_eq!(*cache.read().unwrap().unwrap(), *raw);
        assert_eq!(authenticator.requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn conditional_invalidation_reports_busy_and_legacy_is_local_only() {
        let mut server = mockito::Server::new_async().await;
        let cache = Arc::new(FakeAccessCache::default());
        let clock = Arc::new(FakeCacheClock::new());
        let authenticator = Arc::new(FakeAuthenticator::default());
        cache_mint_mocks(&mut server, &clock).await;
        let session = cache_test_session(
            &server,
            &cache,
            &clock,
            &authenticator,
            DEFAULT_ACCESS_TOKEN_CACHE_TTL,
        );
        let rejected = session.access_credentials().await.unwrap();
        let lock = cache.lock().unwrap();
        assert!(
            session
                .invalidate_rejected_access_token(&rejected)
                .await
                .is_err()
        );
        drop(lock);
        // The rejected generation is remembered even if the backend deletion
        // failed, but a fresh mint is reused locally even if its write fails.
        cache.fail_write.store(true, Ordering::SeqCst);
        let fresh = session.access_credentials().await.unwrap();
        assert_ne!(fresh, rejected);
        assert_eq!(session.access_credentials().await.unwrap(), fresh);
        cache.fail_write.store(false, Ordering::SeqCst);
        let raw = cache.read().unwrap();
        session.invalidate().await;
        assert_eq!(cache.read().unwrap(), raw);
        let fresh = session.access_credentials().await.unwrap();
        let mut wrong_consumer = fresh.clone();
        wrong_consumer.consumer_key = "other-consumer".into();
        assert!(
            !session
                .invalidate_rejected_access_token(&wrong_consumer)
                .await
                .unwrap()
        );
        assert_eq!(session.access_credentials().await.unwrap(), fresh);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn passwd_home_lookup_returns_owned_path() {
        use nix::unistd::{Uid, User};

        let uid = Uid::effective();
        let expected = User::from_uid(uid).unwrap().unwrap().dir;
        let actual = passwd_home_directory(uid.as_raw());
        if expected.as_os_str().is_empty() || expected == Path::new("/") {
            assert!(actual.is_err());
        } else {
            assert_eq!(actual.unwrap(), expected);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn passwd_home_lookup_rejects_missing_user() {
        // Linux reserves (uid_t)-1 as an invalid UID.
        assert!(passwd_home_directory(u32::MAX).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_namespace_child() {
        let Some(home_dir) = std::env::var_os("LPCLI_TEST_HOME_DIR") else {
            return;
        };
        let run_user_root = std::env::var_os("LPCLI_TEST_RUN_USER_ROOT").unwrap();
        let expected_passwd_home = std::env::var_os("LPCLI_TEST_PASSWD_HOME");
        // SAFETY: geteuid has no memory safety requirements.
        let uid = unsafe { libc::geteuid() };
        if let Some(expected_passwd_home) = expected_passwd_home {
            assert_eq!(
                passwd_home_directory(uid).unwrap().as_os_str(),
                expected_passwd_home
            );
        }
        let dir = trusted_coordination_directory_in(
            Some(Path::new(&home_dir)),
            Path::new(&run_user_root),
            uid,
            uid,
        )
        .unwrap();
        assert_eq!(dir, Path::new(&home_dir).join(".cache").join("lpcli"));
        assert!(secure_cache_lock(&dir).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_lock_namespace_ignores_xdg_home_and_tmpdir() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        if is_sudo() {
            return;
        }
        // SAFETY: geteuid has no memory safety requirements.
        let uid = unsafe { libc::geteuid() };
        let passwd_home = passwd_home_directory(uid).ok();

        // Isolated home and /run/user fixtures so parallel tests never contend
        // on host locks or mutate user directories.
        let home_dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(home_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let missing_run_user = tempfile::tempdir().unwrap();
        let missing_run_user_path = missing_run_user.path().join("missing-run-user");

        // Primary home-cache path works with no `/run/user` at all.
        let trusted = trusted_coordination_directory_in(
            Some(home_dir.path()),
            &missing_run_user_path,
            uid,
            uid,
        )
        .unwrap();
        assert_eq!(trusted, home_dir.path().join(".cache").join("lpcli"));

        // Falls back to `/run/user/<uid>` if home is untrusted or unavailable.
        let run_user_root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(run_user_root.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let runtime = run_user_root.path().join(uid.to_string());
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();

        let bad_home = tempfile::tempdir().unwrap();
        std::fs::set_permissions(bad_home.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(trusted_home_cache_directory(bad_home.path(), uid).is_err());
        assert!(trusted_home_cache_directory(Path::new("/"), uid).is_err());
        let symlink_home_root = tempfile::tempdir().unwrap();
        let symlink_home = symlink_home_root.path().join("home-link");
        symlink(home_dir.path(), &symlink_home).unwrap();
        assert!(trusted_home_cache_directory(&symlink_home, uid).is_err());

        assert_eq!(
            trusted_coordination_directory_in(
                Some(bad_home.path()),
                run_user_root.path(),
                uid,
                uid,
            )
            .unwrap(),
            runtime
        );
        assert_eq!(
            trusted_coordination_directory_in(None, run_user_root.path(), uid, uid).unwrap(),
            runtime
        );

        assert!(
            trusted_runtime_directory_in(run_user_root.path(), uid.wrapping_add(1), uid).is_err()
        );
        std::fs::set_permissions(run_user_root.path(), std::fs::Permissions::from_mode(0o775))
            .unwrap();
        assert!(trusted_runtime_directory_in(run_user_root.path(), uid, uid).is_err());
        std::fs::set_permissions(run_user_root.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let symlink_root = tempfile::tempdir().unwrap();
        let symlink_path = symlink_root.path().join("run-user-link");
        symlink(run_user_root.path(), &symlink_path).unwrap();
        assert!(trusted_runtime_directory_in(&symlink_path, uid, uid).is_err());

        let lock = secure_cache_lock(&trusted).unwrap();
        for _ in 0..2 {
            let env_root = tempfile::tempdir().unwrap();
            std::fs::set_permissions(env_root.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "hardware_tokens::tests::cache_namespace_child"])
                .env("LPCLI_TEST_HOME_DIR", home_dir.path())
                .env("LPCLI_TEST_RUN_USER_ROOT", &missing_run_user_path)
                .env("XDG_CONFIG_HOME", env_root.path())
                .env("XDG_RUNTIME_DIR", env_root.path())
                .env("HOME", env_root.path())
                .env("TMPDIR", env_root.path());
            if let Some(ref pw_home) = passwd_home {
                cmd.env("LPCLI_TEST_PASSWD_HOME", pw_home);
            } else {
                cmd.env_remove("LPCLI_TEST_PASSWD_HOME");
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(std::fs::read_dir(env_root.path()).unwrap().count(), 0);
        }
        drop(lock);
        drop(wait_for_cache_lock_in_test(&trusted));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_directory_with_retry_tolerates_brief_contention_and_fails_unavailable_immediately() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-parent").join("lpcli");
        let start = std::time::Instant::now();
        assert_eq!(
            lock_directory_with_retry(&missing).err(),
            Some(LockFailure::Unavailable)
        );
        assert!(start.elapsed() < Duration::from_millis(50));

        let runtime = directory.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let lock = secure_cache_lock(&runtime).unwrap();
        let runtime_bg = runtime.clone();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(lock);
        });
        let reacquired = lock_directory_with_retry(&runtime_bg);
        assert!(reacquired.is_ok());
        drop(reacquired);
        releaser.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_lock_child() {
        let Some(path) = std::env::var_os("LPCLI_TEST_CACHE_LOCK") else {
            return;
        };
        if std::env::var_os("LPCLI_TEST_CACHE_LOCK_EXPECT_FREE").is_some() {
            drop(wait_for_cache_lock_in_test(Path::new(&path)));
        } else {
            assert!(secure_cache_lock(Path::new(&path)).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    fn wait_for_cache_lock_in_test(directory: &Path) -> Box<dyn CacheLock> {
        // flock belongs to the open file description. Parallel tests spawning
        // children can transiently inherit it between fork and exec, even with
        // O_CLOEXEC, so closing the parent's descriptor need not release it
        // immediately. Only this valid-fixture test waits; `secure_cache_lock` stays
        // nonblocking and every attempt still checks ownership and permissions.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match secure_cache_lock(directory) {
                Ok(lock) => return lock,
                Err(error) if std::time::Instant::now() >= deadline => {
                    panic!("test fixture lock remained unavailable after two seconds: {error}");
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_lock_inherited_before_exec_releases_on_exec() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixStream;
        use std::os::unix::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let lock = secure_cache_lock(directory.path()).unwrap();
        let (mut ready_reader, ready_writer) = UnixStream::pair().unwrap();
        let (mut release_writer, release_reader) = UnixStream::pair().unwrap();
        ready_reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        release_reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let executable = std::env::current_exe().unwrap();
        let path = directory.path().to_owned();
        let child = std::thread::spawn(move || {
            let ready_fd = ready_writer.as_raw_fd();
            let release_fd = release_reader.as_raw_fd();
            let mut command = std::process::Command::new(executable);
            command
                .args(["--exact", "hardware_tokens::tests::cache_lock_child"])
                .env("LPCLI_TEST_CACHE_LOCK", path)
                .env("LPCLI_TEST_CACHE_LOCK_EXPECT_FREE", "1");
            // SAFETY: after fork the callback only performs async-signal-safe
            // read/write syscalls with live socket descriptors and fixed-code
            // I/O errors. It uses no locks, logging or allocation. The owning
            // UnixStreams remain alive until the subprocess has finished.
            unsafe {
                command.pre_exec(move || {
                    let ready = [1u8];
                    if libc::write(ready_fd, ready.as_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                    let mut release = [0u8];
                    if libc::read(release_fd, release.as_mut_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                });
            }
            let output = command.output();
            drop((ready_writer, release_reader));
            output
        });

        // Freeze a real Command child after fork but before exec, then close
        // the parent lock. This deterministically reproduces the transient
        // retention which an unrelated parallel test can otherwise cause.
        ready_reader.read_exact(&mut [0u8]).unwrap();
        drop(lock);
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join("hardware-access-token.lock"))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(probe);
        assert!(secure_cache_lock(directory.path()).is_err());
        release_writer.write_all(&[1]).unwrap();
        let output = child.join().unwrap().unwrap();
        // The exec'd child must acquire the lock before exiting: this also
        // verifies production's CLOEXEC behavior, not just exit-time cleanup.
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        drop(wait_for_cache_lock_in_test(directory.path()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_lock_serializes_processes_and_rejects_unsafe_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let lock = secure_cache_lock(directory.path()).unwrap();
        assert!(secure_cache_lock(directory.path()).is_err());
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "hardware_tokens::tests::cache_lock_child"])
            .env("LPCLI_TEST_CACHE_LOCK", directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        drop(lock);
        drop(wait_for_cache_lock_in_test(directory.path()));
        let path = directory.path().join("hardware-access-token.lock");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        // Never lock this replacement inode: otherwise a transiently inherited
        // lock could make the unsafe-permissions assertion pass for the wrong
        // reason. Descriptors for the original inode cannot lock the new one.
        std::fs::remove_file(&path).unwrap();
        drop(std::fs::File::create(&path).unwrap());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(secure_cache_lock(directory.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        symlink(directory.path().join("missing"), &path).unwrap();
        assert!(secure_cache_lock(directory.path()).is_err());
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
        let cache = FakeAccessCache::default();
        assert_eq!(store.load().unwrap(), None);

        let credentials = HardwareTokenCredentials::new(grant(), HARDWARE_CONSUMER_KEY).unwrap();
        let initial_epoch = cache.generation().unwrap();
        cache.write("cached-before-save").unwrap();
        assert_eq!(
            store.save_with_cache(&credentials, &cache).unwrap(),
            StoredIn::File(path.clone())
        );
        assert_eq!(store.load().unwrap(), Some(credentials));
        let after_save_epoch = cache.generation().unwrap();
        assert_ne!(after_save_epoch, initial_epoch);
        assert!(cache.read().unwrap().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files remain");

        cache.write("cached-before-delete").unwrap();
        store.delete_with_cache(&cache).unwrap();
        assert_eq!(store.load().unwrap(), None);
        assert_ne!(cache.generation().unwrap(), after_save_epoch);
        assert!(cache.read().unwrap().is_none());
        store.delete_with_cache(&cache).unwrap();
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

    // -- Registration --------------------------------------------------------

    const NEW_CREDENTIAL_ID: &[u8] = b"new-credential";
    const REGISTRATION_TEST_CHILD: &str = "LPCLI_REGISTRATION_TEST_CHILD";

    // Isolate the storage override from other concurrently running tests.
    fn registration_in_store(test_name: &str) -> bool {
        if std::env::var(REGISTRATION_TEST_CHILD).as_deref() == Ok(test_name) {
            return true;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grant.json");
        let store = GrantStore::File(path.clone());
        let original = HardwareTokenCredentials::new(
            grant()
                .with_credential_id(&encode_base64url(CREDENTIAL_ID))
                .unwrap(),
            HARDWARE_CONSUMER_KEY,
        )
        .unwrap();
        store.save_raw(&original).unwrap();
        let original_bytes = std::fs::read(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env(REGISTRATION_TEST_CHILD, test_name)
            .env(CREDENTIALS_FILE_ENV, &path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "registration subprocess failed:\n{stdout}\n{stderr}"
        );
        assert!(stdout.contains("running 1 test"));
        assert_eq!(store.load().unwrap().as_ref(), Some(&original));
        assert!(std::fs::read(&path).unwrap() == original_bytes);
        false
    }

    async fn registration_mocks(
        server: &mut mockito::ServerGuard,
        consumer: &str,
        token: &str,
    ) -> Vec<mockito::Mock> {
        let person = format!("{}/devel/~example-user", server.url());
        let location = format!("{person}/+authenticator/42");
        let auth = Matcher::Regex(format!(
            r#"oauth_consumer_key="{consumer}", oauth_token="{token}""#
        ));
        let client_data = client_data_json("webauthn.create", &[9; 32], WEB_ROOT).unwrap();
        let credential = json!({
            "id": encode_base64url(NEW_CREDENTIAL_ID),
            "rawId": encode_base64url(NEW_CREDENTIAL_ID),
            "type": "public-key",
            "response": {
                "clientDataJSON": encode_base64url(&client_data),
                "attestationObject": encode_base64url(b"fake-attestation"),
            },
        });
        // The Text parameter contains a JSON string whose contents are JSON.
        let response = serde_json::to_string(&credential.to_string()).unwrap();
        let body = format!(
            "ws.op=completeWebAuthnRegistration&response={}&name=new-key",
            url::form_urlencoded::byte_serialize(response.as_bytes()).collect::<String>()
        );
        let options = json!({"publicKey": {
            "rp": {"id": "launchpad.net", "name": "Launchpad"},
            "user": {"id": encode_base64url(b"user"), "name": "example-user", "displayName": "Example"},
            "challenge": encode_base64url(&[9; 32]),
            "pubKeyCredParams": [{"type": "public-key", "alg": -7}],

        }});
        vec![
            server
                .mock("GET", "/devel/")
                .match_header("authorization", auth.clone())
                .with_body(json!({"me_link": person}).to_string())
                .create_async()
                .await,
            server
                .mock("GET", "/devel/~example-user")
                .match_header("authorization", auth.clone())
                .with_body(json!({"name": "example-user", "self_link": person}).to_string())
                .create_async()
                .await,
            server
                .mock("POST", "/devel/~example-user")
                .match_header("authorization", auth.clone())
                .match_header("cookie", Matcher::Missing)
                .match_body("ws.op=beginWebAuthnRegistration")
                .with_header("set-cookie", "lp_session=abc123; Path=/; HttpOnly")
                .with_body(options.to_string())
                .create_async()
                .await,
            server
                .mock("POST", "/devel/~example-user")
                .match_header("authorization", auth.clone())
                .match_header("cookie", "lp_session=abc123")
                .match_body(body.as_str())
                .with_status(201)
                .with_header("location", &location)
                .create_async()
                .await,
            server
                .mock("GET", "/devel/~example-user/+authenticator/42")
                .match_header("authorization", auth)
                .with_body(json!({"id": 42, "name": "new-key"}).to_string())
                .create_async()
                .await,
        ]
    }

    fn new_registration_key() -> Arc<FakeAuthenticator> {
        Arc::new(FakeAuthenticator {
            credential_id: Some(NEW_CREDENTIAL_ID.to_vec()),
            ..Default::default()
        })
    }

    fn assert_new_registration(key: &FakeAuthenticator) {
        assert!(key.requests.lock().unwrap().is_empty());
        let registrations = key.registrations.lock().unwrap();
        assert_eq!(registrations.len(), 1);

        let data = client_data_json("webauthn.create", &[9; 32], WEB_ROOT).unwrap();
        assert_eq!(
            registrations[0].client_data_hash,
            <[u8; 32]>::from(Sha256::digest(data))
        );
    }

    #[tokio::test]
    async fn registration_with_hardware_session_uses_distinct_keys_and_preserves_grant() {
        if !registration_in_store(
            "hardware_tokens::tests::registration_with_hardware_session_uses_distinct_keys_and_preserves_grant",
        ) {
            return;
        }
        let mut server = mockito::Server::new_async().await;
        let challenge = server
            .mock("POST", "/+hw-challenge")
            .match_body("oauth_token=refresh-token&oauth_token_secret=refresh-secret")
            .with_body(challenge_body("launchpad.net", "discouraged"))
            .expect(1)
            .create_async()
            .await;
        let assertion_data = client_data_json("webauthn.get", &[0xAB; 32], WEB_ROOT).unwrap();
        let assertion = assertion_json(
            &Assertion {
                credential_id: CREDENTIAL_ID.to_vec(),
                authenticator_data: b"authenticator-data".to_vec(),
                signature: b"signature".to_vec(),
                user_handle: None,
            },
            &assertion_data,
        )
        .to_string();
        let exchange = server
            .mock("POST", "/+hw-access-token")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("oauth_token".into(), "refresh-token".into()),
                Matcher::UrlEncoded("oauth_token_secret".into(), "refresh-secret".into()),
                Matcher::UrlEncoded("challenge_id".into(), "7".into()),
                Matcher::UrlEncoded("assertion".into(), assertion),
            ]))
            .with_body(token_body("minted-access", TimeDelta::hours(1)))
            .expect(1)
            .create_async()
            .await;
        let mocks = registration_mocks(&mut server, HARDWARE_CONSUMER_KEY, "minted-access").await;
        let old_key = Arc::new(FakeAuthenticator::default());
        let original = GrantStore::from_env().load().unwrap().unwrap();
        let session = Arc::new(session(&server, old_key.clone(), original.grant().clone()));
        let client = LaunchpadClient::with_hardware_token(Arc::clone(&session))
            .with_base_url(format!("{}/devel", server.url()));
        let new_key = new_registration_key();
        let result = register_hardware_key(&client, "new-key", new_key.clone())
            .await
            .unwrap();

        assert_eq!(result.id, Some(42));
        for mock in mocks {
            mock.assert_async().await;
        }
        challenge.assert_async().await;
        exchange.assert_async().await;
        assert_eq!(session.credentials(), &original);
        assert!(old_key.registrations.lock().unwrap().is_empty());
        let assertions = old_key.requests.lock().unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(
            assertions[0].allow_credentials,
            vec![CREDENTIAL_ID.to_vec()]
        );
        assert_eq!(
            assertions[0].client_data_hash,
            <[u8; 32]>::from(Sha256::digest(assertion_data))
        );
        assert_new_registration(&new_key);
        assert_ne!(
            assertions[0].client_data_hash,
            new_key.registrations.lock().unwrap()[0].client_data_hash
        );
    }

    #[tokio::test]
    async fn registration_with_classic_credentials_needs_no_existing_key() {
        if !registration_in_store(
            "hardware_tokens::tests::registration_with_classic_credentials_needs_no_existing_key",
        ) {
            return;
        }
        let mut server = mockito::Server::new_async().await;
        let challenge = server
            .mock("POST", "/+hw-challenge")
            .expect(0)
            .create_async()
            .await;
        let exchange = server
            .mock("POST", "/+hw-access-token")
            .expect(0)
            .create_async()
            .await;
        let mocks = registration_mocks(&mut server, "classic-consumer", "classic-access").await;
        let client = LaunchpadClient::new(Some(Credentials::new(
            "classic-consumer",
            "classic-access",
            "classic-secret",
        )))
        .with_base_url(format!("{}/devel", server.url()));
        let new_key = new_registration_key();
        let result = register_hardware_key(&client, "new-key", new_key.clone())
            .await
            .unwrap();

        assert_eq!(result.id, Some(42));
        for mock in mocks {
            mock.assert_async().await;
        }
        challenge.assert_async().await;
        exchange.assert_async().await;
        assert_new_registration(&new_key);
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
