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
    /// Prepare to authenticate with an already-registered key, before USB
    /// devices are enumerated. The default does not prompt.
    fn prepare_assertion(&self) -> Result<()> {
        Ok(())
    }

    /// Prepare to register a new key, before USB devices are enumerated.
    /// Interactive implementations can wait here for the user to swap keys
    /// after authenticating with their existing one. The default does not prompt.
    fn prepare_registration(&self) -> Result<()> {
        Ok(())
    }

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

/// Prompts on the terminal. Messages go to stderr so stdout stays usable
/// for command output. Registration confirmation requires stdin to be a
/// terminal; piped or redirected stdin is rejected before prompting.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleInteraction;

impl Interaction for ConsoleInteraction {
    fn prepare_assertion(&self) -> Result<()> {
        eprintln!(
            "Authentication: use the security key bound to your stored grant, not the new key."
        );
        Ok(())
    }

    fn prepare_registration(&self) -> Result<()> {
        use std::io::IsTerminal;

        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Err(LpError::HardwareKey(
                "Hardware key registration requires an interactive session.".to_string(),
            ));
        }
        eprintln!("Registration: insert the NEW security key you want to register.");
        eprintln!(
            "You can now unplug the authentication key, or select the new key if both are connected."
        );
        eprintln!("Press Enter when the new key is connected...");
        wait_for_registration_key(&mut stdin.lock())
    }

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

        read_selection(&mut std::io::stdin().lock(), kind, labels.len())
    }
}

/// Maximum prompt-line length in bytes, including the newline.
const MAX_PROMPT_LINE_BYTES: usize = 1024;

/// Read one complete prompt response without allowing unbounded allocation.
/// This limits bytes, not time: waiting for terminal input may still block.
fn read_prompt_line(input: &mut impl std::io::BufRead) -> Result<String> {
    use std::io::{BufRead, Read};

    let mut line = String::new();
    // Read one extra byte to distinguish a full-sized line from oversized
    // input. Never drain the rest of an invalid line: it may be endless.
    if input
        .take((MAX_PROMPT_LINE_BYTES + 1) as u64)
        .read_line(&mut line)?
        == 0
    {
        return Err(LpError::HardwareKeyCancelled);
    }
    if line.len() > MAX_PROMPT_LINE_BYTES {
        return Err(LpError::HardwareKey(format!(
            "Prompt response exceeds {MAX_PROMPT_LINE_BYTES} bytes."
        )));
    }
    // Take's artificial EOF, or actual EOF after partial input, must not be
    // treated as pressing Enter.
    if !line.ends_with('\n') {
        return Err(LpError::HardwareKey(
            "Prompt response must end with Enter (a newline).".to_string(),
        ));
    }
    Ok(line)
}

fn read_selection(input: &mut impl std::io::BufRead, kind: &str, count: usize) -> Result<usize> {
    read_prompt_line(input)?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|choice| (1..=count).contains(choice))
        .map(|choice| choice - 1)
        .ok_or_else(|| LpError::HardwareKey(format!("Invalid {kind} selection.")))
}

fn wait_for_registration_key(input: &mut impl std::io::BufRead) -> Result<()> {
    read_prompt_line(input).map(|_| ())
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
/// asked to choose one. Authentication and registration have separate
/// selections: the default console interaction pauses before registration
/// so an authentication key can be swapped for the new key.
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
        purpose: &str,
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
                let index = self.interaction.choose(purpose, &labels)?;
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
        _purpose: &str,
        _f: impl FnOnce(&mut ctap2::Session<'_, ctaphid::Unsupported>) -> Result<R>,
    ) -> Result<R> {
        Err(LpError::HardwareKey(
            "USB security keys are currently only supported on Linux.".to_string(),
        ))
    }
}

impl Authenticator for UsbAuthenticator {
    fn get_assertion(&self, request: &AssertionRequest) -> Result<Assertion> {
        #[cfg(target_os = "linux")]
        self.interaction.prepare_assertion()?;
        self.with_device("authentication security key", |session| {
            session.get_assertion(request)
        })
    }

    fn make_credential(&self, request: &RegistrationRequest) -> Result<Registration> {
        #[cfg(target_os = "linux")]
        self.interaction.prepare_registration()?;
        self.with_device("new security key", |session| {
            session.make_credential(request)
        })
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

    #[cfg(target_os = "linux")]
    #[test]
    fn cancelled_preparation_never_opens_a_device() {
        struct CancelledInteraction;

        impl Interaction for CancelledInteraction {
            fn prepare_registration(&self) -> Result<()> {
                Err(LpError::HardwareKeyCancelled)
            }
            fn prepare_assertion(&self) -> Result<()> {
                Err(LpError::HardwareKeyCancelled)
            }
            fn prompt_touch(&self) {
                panic!("must not start a ceremony");
            }
            fn prompt_uv(&self) {
                panic!("must not start a ceremony");
            }
            fn request_pin(&self, _attempt: u32) -> Result<Zeroizing<String>> {
                panic!("must not start a ceremony");
            }
            fn choose(&self, _kind: &str, _labels: &[String]) -> Result<usize> {
                panic!("must not enumerate devices");
            }
        }

        let authenticator = UsbAuthenticator::new(Arc::new(CancelledInteraction));
        let registration = RegistrationRequest {
            rp_id: "launchpad.net".into(),
            rp_name: "Launchpad".into(),
            user_id: b"user".to_vec(),
            user_name: "user".into(),
            user_display_name: "User".into(),
            client_data_hash: [0; 32],
            algorithms: vec![-7],
            exclude_credentials: Vec::new(),
            resident_key: ResidentKey::Discouraged,
            user_verification: UserVerification::Preferred,
            timeout: Duration::from_secs(1),
        };
        assert!(matches!(
            authenticator.make_credential(&registration),
            Err(LpError::HardwareKeyCancelled)
        ));
        let assertion = AssertionRequest {
            rp_id: registration.rp_id,
            client_data_hash: registration.client_data_hash,
            allow_credentials: Vec::new(),
            user_verification: registration.user_verification,
            timeout: registration.timeout,
        };
        assert!(matches!(
            authenticator.get_assertion(&assertion),
            Err(LpError::HardwareKeyCancelled)
        ));
    }

    #[test]
    fn console_registration_rejects_nonterminal_stdin() {
        use std::io::{IsTerminal, Write};
        use std::process::{Command, Stdio};

        const CHILD_ENV: &str = "LPCLI_TEST_NONTERMINAL_REGISTRATION";
        if std::env::var_os(CHILD_ENV).is_some() {
            assert!(!std::io::stdin().is_terminal());
            let err = ConsoleInteraction.prepare_registration().unwrap_err();
            assert!(matches!(err, LpError::HardwareKey(_)));
            assert_eq!(
                err.to_string(),
                "Security key error: Hardware key registration requires an interactive session."
            );
            return;
        }

        // Run in subprocesses so the test controls real stdin without
        // changing the test runner's file descriptors or environment.
        for piped in [false, true] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "hardware_key::tests::console_registration_rejects_nonterminal_stdin",
                ])
                .env(CHILD_ENV, "1")
                .stdin(if piped { Stdio::piped() } else { Stdio::null() })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            if let Some(mut input) = child.stdin.take() {
                // Even a piped Enter must not bypass the terminal check.
                // The child may already have exited and closed its stdin.
                match input.write_all(b"\n") {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Err(e) => panic!("cannot write test input: {e}"),
                }
            }
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stderr.is_empty(),
                "must reject before displaying prompts"
            );
        }
    }

    #[test]
    fn registration_preparation_requires_confirmation() {
        // Keep the existing workflow: Enter confirms, even after whitespace
        // or other text. Both Unix and Windows line endings are accepted.
        for response in ["\n", "\r\n", "   \n", "ready\n", "prêt\n"] {
            assert!(wait_for_registration_key(&mut response.as_bytes()).is_ok());
        }
        assert!(matches!(
            wait_for_registration_key(&mut &b""[..]),
            Err(LpError::HardwareKeyCancelled)
        ));
    }

    #[test]
    fn device_selection_accepts_normal_input() {
        for (response, expected) in [
            ("1\n", 0),
            ("2\r\n", 1),
            ("  2  \n", 1),
            ("\u{2003}2\u{2003}\n", 1),
            ("002\n", 1),
            ("1000\n", 999),
        ] {
            assert_eq!(
                read_selection(&mut response.as_bytes(), "security key", 1000).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn device_selection_still_rejects_invalid_choices() {
        for response in ["0\n", "3\n", "-1\n", "two\n", "\n", "1 2\n"] {
            let err = read_selection(&mut response.as_bytes(), "security key", 2).unwrap_err();
            assert!(err.to_string().contains("Invalid security key selection."));
        }
        assert!(matches!(
            read_selection(&mut &b""[..], "security key", 2),
            Err(LpError::HardwareKeyCancelled)
        ));
    }

    #[test]
    fn prompts_reject_eof_after_partial_input() {
        for response in ["1", "ready", "\r"] {
            let err = wait_for_registration_key(&mut response.as_bytes()).unwrap_err();
            assert!(err.to_string().contains("newline"));
            let err = read_selection(&mut response.as_bytes(), "security key", 2).unwrap_err();
            assert!(err.to_string().contains("newline"));
        }
    }

    #[test]
    fn prompt_limit_allows_full_sized_complete_lines() {
        let registration = format!("{}\n", "x".repeat(MAX_PROMPT_LINE_BYTES - 1));
        assert!(wait_for_registration_key(&mut registration.as_bytes()).is_ok());
        let selection = format!("1{}\n", " ".repeat(MAX_PROMPT_LINE_BYTES - 2));
        assert_eq!(
            read_selection(&mut selection.as_bytes(), "security key", 2).unwrap(),
            0
        );
        // The limit is measured in bytes, not Unicode characters.
        let unicode = format!("{} \n", "é".repeat((MAX_PROMPT_LINE_BYTES - 2) / 2));
        assert_eq!(unicode.len(), MAX_PROMPT_LINE_BYTES);
        assert!(wait_for_registration_key(&mut unicode.as_bytes()).is_ok());
    }

    #[test]
    fn prompts_reject_oversized_input_without_draining_it() {
        for newline in ["", "\n"] {
            // It starts with a valid choice, but truncation must not accept it.
            let response = format!("1{}{newline}", " ".repeat(MAX_PROMPT_LINE_BYTES * 2));
            let mut input = response.as_bytes();
            let err = read_selection(&mut input, "security key", 2).unwrap_err();
            assert!(err.to_string().contains("exceeds"));
            assert_eq!(input.len(), response.len() - MAX_PROMPT_LINE_BYTES - 1);

            let mut input = response.as_bytes();
            let err = wait_for_registration_key(&mut input).unwrap_err();
            assert!(err.to_string().contains("exceeds"));
            assert_eq!(input.len(), response.len() - MAX_PROMPT_LINE_BYTES - 1);
        }
        // A newline on the extra byte is oversized, not confirmation.
        let response = format!("{}\n", "x".repeat(MAX_PROMPT_LINE_BYTES));
        assert!(wait_for_registration_key(&mut response.as_bytes()).is_err());
    }

    #[test]
    fn endless_input_is_bounded() {
        struct EndlessInput {
            bytes_read: usize,
        }
        impl std::io::Read for EndlessInput {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                // Fail fast if the bound is accidentally removed, rather than
                // letting a regression hang or exhaust memory in this test.
                let remaining = MAX_PROMPT_LINE_BYTES + 1 - self.bytes_read;
                assert!(remaining > 0, "read beyond the prompt limit");
                let n = buf.len().min(remaining);
                buf[..n].fill(b'x');
                self.bytes_read += n;
                Ok(n)
            }
        }
        let mut input = std::io::BufReader::with_capacity(1, EndlessInput { bytes_read: 0 });
        let err = read_prompt_line(&mut input).unwrap_err();
        assert!(err.to_string().contains("exceeds"));
        assert_eq!(input.get_ref().bytes_read, MAX_PROMPT_LINE_BYTES + 1);
    }

    #[test]
    fn successive_prompts_do_not_consume_each_others_input() {
        let mut input = &b"\n 2\r\n"[..];
        wait_for_registration_key(&mut input).unwrap();
        assert_eq!(
            read_selection(&mut input, "new security key", 2).unwrap(),
            1
        );
        assert!(input.is_empty());
    }

    #[test]
    fn prompt_errors_do_not_echo_input() {
        let response = format!("{}\n", "PRIVATE-INPUT".repeat(MAX_PROMPT_LINE_BYTES));
        let err = read_prompt_line(&mut response.as_bytes()).unwrap_err();
        assert!(!err.to_string().contains("PRIVATE-INPUT"));
        let mut invalid_utf8 = &[0xff, b'\n'][..];
        assert!(matches!(
            read_prompt_line(&mut invalid_utf8),
            Err(LpError::Io(_))
        ));
    }

    #[test]
    fn non_interactive_never_prompts() {
        assert!(NonInteractive.prepare_assertion().is_ok());
        assert!(NonInteractive.prepare_registration().is_ok());
        assert!(NonInteractive.request_pin(0).is_err());
        assert!(
            NonInteractive
                .choose("security key", &["a".into()])
                .is_err()
        );
    }
}
