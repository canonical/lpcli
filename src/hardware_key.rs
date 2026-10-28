//! FIDO2 security-key support for hardware-bound API tokens.
//!
//! Launchpad can bind an OAuth refresh grant to a USB FIDO2 security key
//! (see [`crate::hardware_tokens`]).  Exchanging that grant for a short-lived
//! access token, and registering a key in the first place, require WebAuthn
//! ceremonies performed by the key itself.  This module talks to the key.
//!
//! The [`Authenticator`] trait is the boundary between the WebAuthn layer and
//! the hardware: it receives an already-hashed client-data payload and
//! returns what the key signed.  [`UsbAuthenticator`] is the production
//! implementation.  It speaks CTAP2 over CTAPHID directly to the Linux
//! `hidraw` device, the same approach `python-fido2` uses on Linux, so lpcli
//! needs neither `libfido2` nor `libudev` at build or run time.
//!
//! Only USB (HID) authenticators are supported.  NFC authenticators,
//! platform passkeys and U2F-only (CTAP1) keys are not.
//!
//! # Device permissions
//!
//! Talking to a USB key needs read/write access to its `/dev/hidraw*` node.
//! Most distributions grant this to the logged-in user via the FIDO udev
//! rules shipped by `systemd` or `libfido2`.  Avoid rules that grant access
//! to every `hidraw` device.  Running as root is not required.
//!
//! # User interaction
//!
//! [`Interaction`] receives touch prompts, PIN requests and device choices.
//! [`ConsoleInteraction`] prompts on the terminal; [`NonInteractive`] fails
//! instead of prompting, which cannot bypass what the key itself requires.

use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::error::{LpError, Result};

// The CTAP2 stack is only reachable through hidraw, so elsewhere it is
// exercised by tests alone.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod ctap2;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod ctaphid;
#[cfg(target_os = "linux")]
mod hidraw;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod pin;

// ---------------------------------------------------------------------------
// Request and response types
// ---------------------------------------------------------------------------

/// The WebAuthn user-verification requirement of a ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserVerification {
    /// The key must verify the user (PIN or built-in biometrics).
    Required,
    /// Verify the user when the key has verification configured.
    #[default]
    Preferred,
    /// Only test for user presence (a touch).
    Discouraged,
}

impl UserVerification {
    /// Parse the WebAuthn `userVerification` value.
    pub fn from_webauthn(value: &str) -> Option<Self> {
        match value {
            "required" => Some(Self::Required),
            "preferred" => Some(Self::Preferred),
            "discouraged" => Some(Self::Discouraged),
            _ => None,
        }
    }
}

/// The WebAuthn resident-key (discoverable credential) requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResidentKey {
    /// A discoverable credential must be created.
    Required,
    /// Create a discoverable credential when the key supports it.
    Preferred,
    /// Create a non-discoverable credential.
    #[default]
    Discouraged,
}

impl ResidentKey {
    /// Parse the WebAuthn `residentKey` value.
    pub fn from_webauthn(value: &str) -> Option<Self> {
        match value {
            "required" => Some(Self::Required),
            "preferred" => Some(Self::Preferred),
            "discouraged" => Some(Self::Discouraged),
            _ => None,
        }
    }
}

/// A request to sign a WebAuthn assertion (`authenticatorGetAssertion`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionRequest {
    /// The WebAuthn relying party ID, e.g. `launchpad.net`.
    pub rp_id: String,
    /// SHA-256 of the `clientDataJSON` that the key signs over.
    pub client_data_hash: [u8; 32],
    /// Credential IDs the relying party accepts.  Empty means any
    /// discoverable credential for `rp_id`.
    pub allow_credentials: Vec<Vec<u8>>,
    /// The user-verification requirement.
    pub user_verification: UserVerification,
    /// How long to wait for the user to touch the key.
    pub timeout: Duration,
}

/// A signed WebAuthn assertion returned by a security key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    /// The ID of the credential that signed the assertion.
    pub credential_id: Vec<u8>,
    /// The raw authenticator data.
    pub authenticator_data: Vec<u8>,
    /// The signature over `authenticator_data || client_data_hash`.
    pub signature: Vec<u8>,
    /// The user handle, returned for discoverable credentials.
    pub user_handle: Option<Vec<u8>>,
}

/// A request to create a credential (`authenticatorMakeCredential`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    /// The WebAuthn relying party ID, e.g. `launchpad.net`.
    pub rp_id: String,
    /// The relying party's display name.
    pub rp_name: String,
    /// The opaque user handle chosen by the relying party.
    pub user_id: Vec<u8>,
    /// The user's account name.
    pub user_name: String,
    /// The user's display name.
    pub user_display_name: String,
    /// SHA-256 of the `clientDataJSON` that the key signs over.
    pub client_data_hash: [u8; 32],
    /// COSE algorithm identifiers acceptable to the relying party, in order
    /// of preference (e.g. `-7` for ES256).
    pub algorithms: Vec<i64>,
    /// Credential IDs that are already registered and must not be reused.
    pub exclude_credentials: Vec<Vec<u8>>,
    /// The resident-key requirement.
    pub resident_key: ResidentKey,
    /// The user-verification requirement.
    pub user_verification: UserVerification,
    /// How long to wait for the user to touch the key.
    pub timeout: Duration,
}

/// A newly created credential returned by a security key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// The ID of the new credential.
    pub credential_id: Vec<u8>,
    /// The CBOR-encoded WebAuthn attestation object.
    pub attestation_object: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Performs WebAuthn ceremonies with a security key.
///
/// Implementations are blocking; async callers should run them on a
/// blocking thread (e.g. with [`tokio::task::spawn_blocking`]).
pub trait Authenticator: Send + Sync {
    /// Sign an assertion over `request.client_data_hash`.
    fn get_assertion(&self, request: &AssertionRequest) -> Result<Assertion>;

    /// Create a new credential.
    fn make_credential(&self, request: &RegistrationRequest) -> Result<Registration>;
}

/// Hooks for prompting the user during a security-key ceremony.
pub trait Interaction: Send + Sync {
    /// Ask the user to touch the key.  Called when the key reports that it
    /// is waiting for user presence.
    fn prompt_touch(&self);

    /// Tell the user that the key will verify them with built-in
    /// verification, such as a fingerprint.
    fn prompt_uv(&self);

    /// Ask for the key's PIN.  `attempt` is zero for the first request and
    /// increases after each incorrect PIN.
    fn request_pin(&self, attempt: u32) -> Result<Zeroizing<String>>;

    /// Ask the user to choose one of several `labels` describing a
    /// `kind` of item (e.g. "security key"), returning its index.
    fn choose(&self, kind: &str, labels: &[String]) -> Result<usize>;
}

/// Prompts on the terminal.  Messages go to stderr so stdout stays usable
/// for command output.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleInteraction;

impl Interaction for ConsoleInteraction {
    fn prompt_touch(&self) {
        eprintln!("Touch your security key to continue...");
    }

    fn prompt_uv(&self) {
        eprintln!("Verify yourself on your security key (for example, with your fingerprint)...");
    }

    fn request_pin(&self, attempt: u32) -> Result<Zeroizing<String>> {
        if attempt > 0 {
            eprintln!("Incorrect PIN; try again.");
        }
        let pin = Zeroizing::new(
            rpassword::prompt_password("Security key PIN: ")
                .map_err(|e| LpError::HardwareKey(format!("Cannot read the PIN: {e}")))?,
        );
        if pin.is_empty() {
            return Err(LpError::HardwareKeyCancelled);
        }
        Ok(pin)
    }

    fn choose(&self, kind: &str, labels: &[String]) -> Result<usize> {
        use std::io::Write;

        eprintln!("Several {kind}s are available:");
        for (index, label) in labels.iter().enumerate() {
            eprintln!("  {}. {label}", index + 1);
        }
        eprint!("Choose a {kind} [1-{}]: ", labels.len());
        let _ = std::io::stderr().flush();

        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Err(LpError::HardwareKeyCancelled);
        }
        line.trim()
            .parse::<usize>()
            .ok()
            .filter(|choice| (1..=labels.len()).contains(choice))
            .map(|choice| choice - 1)
            .ok_or_else(|| LpError::HardwareKey(format!("Invalid {kind} selection.")))
    }
}

/// Never prompts: PIN requests and choices fail.  The key may still need a
/// touch, which no software can bypass.
#[derive(Debug, Clone, Copy, Default)]
pub struct NonInteractive;

impl Interaction for NonInteractive {
    fn prompt_touch(&self) {}

    fn prompt_uv(&self) {}

    fn request_pin(&self, _attempt: u32) -> Result<Zeroizing<String>> {
        Err(LpError::HardwareKey(
            "The security key needs a PIN, but prompting is disabled.".to_string(),
        ))
    }

    fn choose(&self, kind: &str, _labels: &[String]) -> Result<usize> {
        Err(LpError::HardwareKey(format!(
            "Several {kind}s are available, but prompting is disabled."
        )))
    }
}

// ---------------------------------------------------------------------------
// USB implementation
// ---------------------------------------------------------------------------

/// A USB (HID) FIDO2 security key, found by scanning `/dev/hidraw*`.
///
/// Each ceremony opens the key, runs the CTAP2 command and closes the device
/// again, even on failure.  When several keys are plugged in, the user is
/// asked to choose one.
#[derive(Clone)]
pub struct UsbAuthenticator {
    interaction: Arc<dyn Interaction>,
}

impl std::fmt::Debug for UsbAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbAuthenticator").finish_non_exhaustive()
    }
}

impl Default for UsbAuthenticator {
    fn default() -> Self {
        Self::new(Arc::new(ConsoleInteraction))
    }
}

impl UsbAuthenticator {
    /// Create an authenticator that prompts through `interaction`.
    pub fn new(interaction: Arc<dyn Interaction>) -> Self {
        Self { interaction }
    }

    /// Open the (chosen) security key and run `f` with a CTAP2 session.
    #[cfg(target_os = "linux")]
    fn with_device<R>(
        &self,
        f: impl FnOnce(&mut ctap2::Session<'_, hidraw::HidrawDevice>) -> Result<R>,
    ) -> Result<R> {
        let devices = hidraw::list_fido_devices().map_err(hidraw::access_error)?;
        let index = match devices.len() {
            0 => {
                return Err(LpError::HardwareKey(
                    "No USB FIDO2 security key found.".to_string(),
                ));
            }
            1 => 0,
            _ => {
                let labels: Vec<String> = devices.iter().map(|d| d.label()).collect();
                let index = self.interaction.choose("security key", &labels)?;
                if index >= devices.len() {
                    return Err(LpError::HardwareKey(
                        "Invalid device selection.".to_string(),
                    ));
                }
                index
            }
        };

        let device =
            hidraw::HidrawDevice::open(&devices[index].path).map_err(hidraw::access_error)?;
        let mut channel = ctaphid::Channel::open(device)?;
        let mut session = ctap2::Session::new(&mut channel, self.interaction.as_ref())?;
        f(&mut session)
    }

    #[cfg(not(target_os = "linux"))]
    fn with_device<R>(
        &self,
        _f: impl FnOnce(&mut ctap2::Session<'_, ctaphid::Unsupported>) -> Result<R>,
    ) -> Result<R> {
        Err(LpError::HardwareKey(
            "USB security keys are currently only supported on Linux.".to_string(),
        ))
    }
}

impl Authenticator for UsbAuthenticator {
    fn get_assertion(&self, request: &AssertionRequest) -> Result<Assertion> {
        self.with_device(|session| session.get_assertion(request))
    }

    fn make_credential(&self, request: &RegistrationRequest) -> Result<Registration> {
        self.with_device(|session| session.make_credential(request))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_verification_parses_webauthn_values() {
        assert_eq!(
            UserVerification::from_webauthn("required"),
            Some(UserVerification::Required)
        );
        assert_eq!(
            UserVerification::from_webauthn("discouraged"),
            Some(UserVerification::Discouraged)
        );
        assert_eq!(UserVerification::from_webauthn("sometimes"), None);
    }

    #[test]
    fn non_interactive_never_prompts() {
        assert!(NonInteractive.request_pin(0).is_err());
        assert!(
            NonInteractive
                .choose("security key", &["a".into()])
                .is_err()
        );
    }
}
