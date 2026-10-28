//! CTAP2 commands over a CTAPHID channel.
//!
//! Requests and responses are CBOR maps keyed by small integers.  Requests
//! use CTAP2 canonical CBOR, which for the maps built here means keys in
//! ascending order; every map below is written in that order.
//!
//! User verification follows python-fido2 (used by launchpadlib) so both
//! clients prompt in the same situations:
//!
//! * `required` always verifies the user; `preferred` verifies when the key
//!   has a PIN or built-in verification configured; `discouraged` only asks
//!   for a touch.
//! * Creating a credential on a key with verification configured always
//!   verifies the user, because CTAP 2.0 keys demand it.
//! * Built-in verification (e.g. a fingerprint) is preferred over a PIN.
//!
//! See section 6, "Authenticator API", of the CTAP 2.1 specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ciborium::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::ctaphid::{Channel, KEEPALIVE_UP_NEEDED, Transport};
use super::pin::{self, PinProtocol};
use super::{
    Assertion, AssertionRequest, Interaction, Registration, RegistrationRequest, ResidentKey,
    UserVerification,
};
use crate::error::{LpError, Result};

const CMD_MAKE_CREDENTIAL: u8 = 0x01;
const CMD_GET_ASSERTION: u8 = 0x02;
const CMD_GET_INFO: u8 = 0x04;
const CMD_CLIENT_PIN: u8 = 0x06;

const PIN_GET_KEY_AGREEMENT: i64 = 0x02;
const PIN_GET_PIN_TOKEN: i64 = 0x05;
const PIN_GET_UV_TOKEN_WITH_PERMISSIONS: i64 = 0x06;
const PIN_GET_PIN_TOKEN_WITH_PERMISSIONS: i64 = 0x09;

const PERMISSION_MAKE_CREDENTIAL: i64 = 0x01;
const PERMISSION_GET_ASSERTION: i64 = 0x02;

/// COSE key parameters for the platform's key-agreement key.
const COSE_KTY_EC2: i64 = 2;
const COSE_ALG_ECDH_ES_HKDF_256: i64 = -25;
const COSE_CRV_P256: i64 = 1;

/// Timeout for commands that need no user interaction.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PIN_ATTEMPTS: u32 = 3;

const ERR_CREDENTIAL_EXCLUDED: u8 = 0x19;
const ERR_OPERATION_DENIED: u8 = 0x27;
const ERR_KEEPALIVE_CANCEL: u8 = 0x2D;
const ERR_NO_CREDENTIALS: u8 = 0x2E;
const ERR_USER_ACTION_TIMEOUT: u8 = 0x2F;
const ERR_PIN_INVALID: u8 = 0x31;
const ERR_PIN_BLOCKED: u8 = 0x32;
const ERR_PIN_AUTH_INVALID: u8 = 0x33;
const ERR_PIN_AUTH_BLOCKED: u8 = 0x34;
const ERR_PIN_NOT_SET: u8 = 0x35;
const ERR_PUAT_REQUIRED: u8 = 0x36;
const ERR_PIN_POLICY_VIOLATION: u8 = 0x37;
const ERR_ACTION_TIMEOUT: u8 = 0x3A;
const ERR_UV_BLOCKED: u8 = 0x3C;
const ERR_UV_INVALID: u8 = 0x3F;

/// A failed CTAP2 command: either a status code from the key, which callers
/// may want to inspect (e.g. to retry a wrong PIN), or any other error.
enum Failure {
    Status(u8),
    Other(LpError),
}

impl From<LpError> for Failure {
    fn from(e: LpError) -> Self {
        Self::Other(e)
    }
}

impl From<Failure> for LpError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Status(code) => status_error(code),
            Failure::Other(e) => e,
        }
    }
}

type CtapResult<T> = std::result::Result<T, Failure>;
type CborMap = Vec<(Value, Value)>;

/// The parts of `authenticatorGetInfo` that drive the ceremony.
#[derive(Debug, Default)]
struct Info {
    options: BTreeMap<String, bool>,
    pin_uv_protocols: Vec<u64>,
}

impl Info {
    fn option(&self, name: &str) -> bool {
        self.options.get(name).copied().unwrap_or(false)
    }

    /// Whether a PIN or built-in user verification is set up.
    fn uv_configured(&self) -> bool {
        ["uv", "clientPin", "bioEnroll"]
            .iter()
            .any(|name| self.option(name))
    }
}

/// How a command proves user verification.
enum UvAuth {
    None,
    /// CTAP 2.0 built-in verification: the `uv` option.
    BuiltIn,
    /// A `pinUvAuthToken` obtained with a PIN or built-in verification.
    Token {
        protocol: PinProtocol,
        token: Zeroizing<Vec<u8>>,
    },
}

/// A CTAP2 session with one security key.
pub(crate) struct Session<'a, T> {
    channel: &'a mut Channel<T>,
    interaction: &'a dyn Interaction,
    info: Info,
}

impl<'a, T: Transport> Session<'a, T> {
    /// Start a session, reading the key's capabilities.
    pub(crate) fn new(
        channel: &'a mut Channel<T>,
        interaction: &'a dyn Interaction,
    ) -> Result<Self> {
        let response = command(channel, interaction, CMD_GET_INFO, None, COMMAND_TIMEOUT)?;
        Ok(Self {
            channel,
            interaction,
            info: parse_info(&response),
        })
    }

    /// Sign an assertion (`authenticatorGetAssertion`).
    pub(crate) fn get_assertion(&mut self, request: &AssertionRequest) -> Result<Assertion> {
        let auth = if self.should_use_uv(request.user_verification, false)? {
            self.uv_auth(PERMISSION_GET_ASSERTION, &request.rp_id, request.timeout)?
        } else {
            UvAuth::None
        };

        let mut params: CborMap = vec![
            (int(1), text(&request.rp_id)),
            (int(2), bytes(&request.client_data_hash)),
        ];
        if !request.allow_credentials.is_empty() {
            let allow_list = request
                .allow_credentials
                .iter()
                .map(|id| credential_descriptor(id))
                .collect();
            params.push((int(3), Value::Array(allow_list)));
        }
        if let Some(options) = options_map(false, matches!(auth, UvAuth::BuiltIn)) {
            params.push((int(5), options));
        }
        if let Some((param, protocol)) = auth_param(&auth, &request.client_data_hash)? {
            params.push((int(6), param));
            params.push((int(7), protocol));
        }

        let response = self.command(CMD_GET_ASSERTION, params, request.timeout)?;
        parse_assertion(&response, &request.allow_credentials)
    }

    /// Create a credential (`authenticatorMakeCredential`).
    pub(crate) fn make_credential(
        &mut self,
        request: &RegistrationRequest,
    ) -> Result<Registration> {
        let resident_key = match request.resident_key {
            ResidentKey::Required if !self.info.option("rk") => {
                return Err(LpError::HardwareKey(
                    "This security key cannot store discoverable credentials.".to_string(),
                ));
            }
            ResidentKey::Required => true,
            ResidentKey::Preferred => self.info.option("rk"),
            ResidentKey::Discouraged => false,
        };
        let auth = if self.should_use_uv(request.user_verification, true)? {
            self.uv_auth(PERMISSION_MAKE_CREDENTIAL, &request.rp_id, request.timeout)?
        } else {
            UvAuth::None
        };

        let rp = Value::Map(vec![
            (text("id"), text(&request.rp_id)),
            (text("name"), text(&request.rp_name)),
        ]);
        let user = Value::Map(vec![
            (text("id"), bytes(&request.user_id)),
            (text("name"), text(&request.user_name)),
            (text("displayName"), text(&request.user_display_name)),
        ]);
        let algorithms = request
            .algorithms
            .iter()
            .map(|&alg| {
                Value::Map(vec![
                    (text("alg"), int(alg)),
                    (text("type"), text("public-key")),
                ])
            })
            .collect();

        let mut params: CborMap = vec![
            (int(1), bytes(&request.client_data_hash)),
            (int(2), rp),
            (int(3), user),
            (int(4), Value::Array(algorithms)),
        ];
        if !request.exclude_credentials.is_empty() {
            let exclude_list = request
                .exclude_credentials
                .iter()
                .map(|id| credential_descriptor(id))
                .collect();
            params.push((int(5), Value::Array(exclude_list)));
        }
        if let Some(options) = options_map(resident_key, matches!(auth, UvAuth::BuiltIn)) {
            params.push((int(7), options));
        }
        if let Some((param, protocol)) = auth_param(&auth, &request.client_data_hash)? {
            params.push((int(8), param));
            params.push((int(9), protocol));
        }

        let response = self.command(CMD_MAKE_CREDENTIAL, params, request.timeout)?;
        parse_registration(&response)
    }

    fn command(&mut self, cmd: u8, params: CborMap, timeout: Duration) -> CtapResult<CborMap> {
        command(
            self.channel,
            self.interaction,
            cmd,
            Some(Value::Map(params)),
            timeout,
        )
    }

    /// Decide whether the ceremony must verify the user.
    fn should_use_uv(&self, requirement: UserVerification, make_credential: bool) -> Result<bool> {
        let configured = self.info.uv_configured();
        let use_uv = match requirement {
            UserVerification::Required => true,
            UserVerification::Preferred => configured,
            UserVerification::Discouraged => false,
        } || self.info.option("alwaysUv")
            || (make_credential && configured && !self.info.option("makeCredUvNotRqd"));

        if use_uv && !configured {
            return Err(LpError::HardwareKey(
                "The security key must verify you, but has no PIN or built-in verification \
                 configured. Set a PIN on the key first."
                    .to_string(),
            ));
        }
        Ok(use_uv)
    }

    /// Obtain user verification, preferring built-in verification to a PIN.
    fn uv_auth(&mut self, permissions: i64, rp_id: &str, timeout: Duration) -> Result<UvAuth> {
        if self.info.option("uv") {
            self.interaction.prompt_uv();
            if self.info.option("pinUvAuthToken") {
                return Ok(self.uv_token(permissions, rp_id, timeout)?);
            }
            return Ok(UvAuth::BuiltIn);
        }

        if self.info.option("clientPin") {
            for attempt in 0..MAX_PIN_ATTEMPTS {
                let pin = self.interaction.request_pin(attempt)?;
                match self.pin_token(&pin, permissions, rp_id) {
                    Err(Failure::Status(ERR_PIN_INVALID)) if attempt + 1 < MAX_PIN_ATTEMPTS => {}
                    result => return Ok(result?),
                }
            }
        }

        Err(LpError::HardwareKey(
            "User verification is not configured on this security key.".to_string(),
        ))
    }

    fn pin_protocol(&self) -> CtapResult<PinProtocol> {
        PinProtocol::select(&self.info.pin_uv_protocols).ok_or_else(|| {
            Failure::Other(LpError::HardwareKey(
                "The security key does not support a known PIN protocol.".to_string(),
            ))
        })
    }

    /// Get the key's agreement key and derive a shared secret, returning the
    /// platform's COSE key to send back and the secret.
    fn key_agreement(&mut self, protocol: PinProtocol) -> CtapResult<(Value, Zeroizing<Vec<u8>>)> {
        let response = self.command(
            CMD_CLIENT_PIN,
            vec![
                (int(1), int(protocol.version())),
                (int(2), int(PIN_GET_KEY_AGREEMENT)),
            ],
            COMMAND_TIMEOUT,
        )?;
        let cose = get(&response, 1)
            .and_then(Value::as_map)
            .ok_or_else(malformed)?;
        let x = get(cose, -2)
            .and_then(Value::as_bytes)
            .ok_or_else(malformed)?;
        let y = get(cose, -3)
            .and_then(Value::as_bytes)
            .ok_or_else(malformed)?;

        let agreement = pin::key_agreement(protocol, x, y)?;
        let platform_key = Value::Map(vec![
            (int(1), int(COSE_KTY_EC2)),
            (int(3), int(COSE_ALG_ECDH_ES_HKDF_256)),
            (int(-1), int(COSE_CRV_P256)),
            (int(-2), bytes(&agreement.public_x)),
            (int(-3), bytes(&agreement.public_y)),
        ]);
        Ok((platform_key, agreement.shared_secret))
    }

    /// Exchange a PIN for a `pinUvAuthToken`.
    fn pin_token(&mut self, pin: &str, permissions: i64, rp_id: &str) -> CtapResult<UvAuth> {
        let protocol = self.pin_protocol()?;
        let (platform_key, shared_secret) = self.key_agreement(protocol)?;

        let pin_hash = Zeroizing::new(Sha256::digest(pin.as_bytes()).to_vec());
        let pin_hash_enc = protocol.encrypt(&shared_secret, &pin_hash[..16])?;

        // CTAP 2.1 keys scope tokens to a permission and relying party;
        // CTAP 2.0 keys only offer the unscoped getPinToken.
        let scoped = self.info.option("pinUvAuthToken");
        let subcommand = if scoped {
            PIN_GET_PIN_TOKEN_WITH_PERMISSIONS
        } else {
            PIN_GET_PIN_TOKEN
        };
        let mut params: CborMap = vec![
            (int(1), int(protocol.version())),
            (int(2), int(subcommand)),
            (int(3), platform_key),
            (int(6), bytes(&pin_hash_enc)),
        ];
        if scoped {
            params.push((int(9), int(permissions)));
            params.push((int(10), text(rp_id)));
        }

        let response = self.command(CMD_CLIENT_PIN, params, COMMAND_TIMEOUT)?;
        self.decrypt_token(protocol, &shared_secret, &response)
    }

    /// Obtain a `pinUvAuthToken` with built-in user verification.
    fn uv_token(&mut self, permissions: i64, rp_id: &str, timeout: Duration) -> CtapResult<UvAuth> {
        let protocol = self.pin_protocol()?;
        let (platform_key, shared_secret) = self.key_agreement(protocol)?;
        let params: CborMap = vec![
            (int(1), int(protocol.version())),
            (int(2), int(PIN_GET_UV_TOKEN_WITH_PERMISSIONS)),
            (int(3), platform_key),
            (int(9), int(permissions)),
            (int(10), text(rp_id)),
        ];
        let response = self.command(CMD_CLIENT_PIN, params, timeout)?;
        self.decrypt_token(protocol, &shared_secret, &response)
    }

    fn decrypt_token(
        &self,
        protocol: PinProtocol,
        shared_secret: &[u8],
        response: &CborMap,
    ) -> CtapResult<UvAuth> {
        let encrypted = get(response, 2)
            .and_then(Value::as_bytes)
            .ok_or_else(malformed)?;
        let token = protocol.decrypt(shared_secret, encrypted)?;
        Ok(UvAuth::Token { protocol, token })
    }
}

/// Send one CTAP2 command and decode the response map.
fn command<T: Transport>(
    channel: &mut Channel<T>,
    interaction: &dyn Interaction,
    cmd: u8,
    params: Option<Value>,
    timeout: Duration,
) -> CtapResult<CborMap> {
    let mut request = vec![cmd];
    if let Some(params) = params {
        ciborium::ser::into_writer(&params, &mut request)
            .map_err(|_| LpError::HardwareKey("Cannot encode the CTAP2 request.".to_string()))?;
    }

    let mut prompted = false;
    let response = channel.cbor(&request, timeout, &mut |status| {
        if status == KEEPALIVE_UP_NEEDED && !prompted {
            prompted = true;
            interaction.prompt_touch();
        }
    })?;

    let (&status, body) = response.split_first().ok_or_else(malformed)?;
    if status != 0 {
        return Err(Failure::Status(status));
    }
    if body.is_empty() {
        return Ok(Vec::new());
    }
    match ciborium::de::from_reader::<Value, _>(body) {
        Ok(Value::Map(map)) => Ok(map),
        _ => Err(malformed().into()),
    }
}

fn parse_info(response: &CborMap) -> Info {
    let options = get(response, 4)
        .and_then(Value::as_map)
        .map(|options| {
            options
                .iter()
                .filter_map(|(k, v)| Some((k.as_text()?.to_string(), v.as_bool()?)))
                .collect()
        })
        .unwrap_or_default();
    let pin_uv_protocols = get(response, 6)
        .and_then(Value::as_array)
        .map(|protocols| {
            protocols
                .iter()
                .filter_map(|v| v.as_integer().and_then(|i| u64::try_from(i).ok()))
                .collect()
        })
        .unwrap_or_default();
    Info {
        options,
        pin_uv_protocols,
    }
}

fn parse_assertion(response: &CborMap, allow_list: &[Vec<u8>]) -> Result<Assertion> {
    let credential_id = match get(response, 1).and_then(Value::as_map) {
        Some(credential) => get_text(credential, "id")
            .and_then(Value::as_bytes)
            .cloned(),
        // The key may omit the credential when exactly one was allowed.
        None if allow_list.len() == 1 => Some(allow_list[0].clone()),
        None => None,
    };
    let user_handle = get(response, 4)
        .and_then(Value::as_map)
        .and_then(|user| get_text(user, "id"))
        .and_then(Value::as_bytes)
        .cloned();

    Ok(Assertion {
        credential_id: credential_id.ok_or_else(malformed)?,
        authenticator_data: required_bytes(response, 2)?,
        signature: required_bytes(response, 3)?,
        user_handle,
    })
}

fn parse_registration(response: &CborMap) -> Result<Registration> {
    let format = get(response, 1)
        .and_then(Value::as_text)
        .ok_or_else(malformed)?;
    let auth_data = required_bytes(response, 2)?;
    let statement = get(response, 3).cloned().ok_or_else(malformed)?;
    let credential_id = attested_credential_id(&auth_data).ok_or_else(|| {
        LpError::HardwareKey("The security key returned no attested credential.".to_string())
    })?;

    // The WebAuthn attestation object uses text keys, in canonical order.
    let attestation = Value::Map(vec![
        (text("fmt"), text(format)),
        (text("attStmt"), statement),
        (text("authData"), Value::Bytes(auth_data)),
    ]);
    let mut attestation_object = Vec::new();
    ciborium::ser::into_writer(&attestation, &mut attestation_object).map_err(|_| malformed())?;

    Ok(Registration {
        credential_id,
        attestation_object,
    })
}

/// Extract the credential ID from authenticator data with attested
/// credential data: rpIdHash (32) ‖ flags (1) ‖ signCount (4) ‖ AAGUID (16)
/// ‖ credentialIdLength (2, big-endian) ‖ credentialId ‖ ...
fn attested_credential_id(auth_data: &[u8]) -> Option<Vec<u8>> {
    const FLAG_ATTESTED_CREDENTIAL_DATA: u8 = 0x40;
    const ID_LENGTH_OFFSET: usize = 53;

    let flags = *auth_data.get(32)?;
    if flags & FLAG_ATTESTED_CREDENTIAL_DATA == 0 {
        return None;
    }
    let length_bytes = auth_data.get(ID_LENGTH_OFFSET..ID_LENGTH_OFFSET + 2)?;
    let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    let start = ID_LENGTH_OFFSET + 2;
    auth_data.get(start..start + length).map(<[u8]>::to_vec)
}

fn options_map(resident_key: bool, builtin_uv: bool) -> Option<Value> {
    let mut options = Vec::new();
    if resident_key {
        options.push((text("rk"), Value::Bool(true)));
    }
    if builtin_uv {
        options.push((text("uv"), Value::Bool(true)));
    }
    (!options.is_empty()).then_some(Value::Map(options))
}

/// The `pinUvAuthParam` and protocol for a command over `client_data_hash`.
fn auth_param(auth: &UvAuth, client_data_hash: &[u8]) -> Result<Option<(Value, Value)>> {
    match auth {
        UvAuth::Token { protocol, token } => Ok(Some((
            Value::Bytes(protocol.authenticate(token, client_data_hash)?),
            int(protocol.version()),
        ))),
        UvAuth::None | UvAuth::BuiltIn => Ok(None),
    }
}

fn credential_descriptor(id: &[u8]) -> Value {
    Value::Map(vec![
        (text("id"), bytes(id)),
        (text("type"), text("public-key")),
    ])
}

fn int(n: i64) -> Value {
    Value::Integer(n.into())
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn bytes(b: &[u8]) -> Value {
    Value::Bytes(b.to_vec())
}

fn get(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter()
        .find(|(k, _)| {
            k.as_integer()
                .is_some_and(|i| i128::from(i) == i128::from(key))
        })
        .map(|(_, v)| v)
}

fn get_text<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

fn required_bytes(map: &[(Value, Value)], key: i64) -> Result<Vec<u8>> {
    get(map, key)
        .and_then(Value::as_bytes)
        .cloned()
        .ok_or_else(malformed)
}

fn malformed() -> LpError {
    LpError::HardwareKey("The security key returned a malformed response.".to_string())
}

/// Explain a CTAP2 status code.
fn status_error(code: u8) -> LpError {
    let message = match code {
        ERR_KEEPALIVE_CANCEL | ERR_OPERATION_DENIED => return LpError::HardwareKeyCancelled,
        ERR_USER_ACTION_TIMEOUT | ERR_ACTION_TIMEOUT => return LpError::HardwareKeyTimeout,
        ERR_PIN_INVALID | ERR_PIN_AUTH_INVALID => "Incorrect security-key PIN.",
        ERR_PIN_BLOCKED => "The PIN is blocked; follow the key vendor's recovery instructions.",
        ERR_PIN_AUTH_BLOCKED => "PIN authentication is temporarily blocked; reconnect the key.",
        ERR_PIN_NOT_SET => "Configure a PIN on the key first.",
        ERR_PUAT_REQUIRED => "The security key requires a PIN for this operation.",
        ERR_PIN_POLICY_VIOLATION => "The PIN does not meet the key's PIN policy.",
        ERR_NO_CREDENTIALS => "This key has no matching credential.",
        ERR_CREDENTIAL_EXCLUDED => "This security key is already registered.",
        ERR_UV_BLOCKED | ERR_UV_INVALID => {
            "Built-in user verification failed; use the key's PIN or try again."
        }
        _ => {
            return LpError::HardwareKey(format!(
                "Security-key operation failed (CTAP2 status 0x{code:02x})."
            ));
        }
    };
    LpError::HardwareKey(message.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use p256::SecretKey;
    use p256::elliptic_curve::sec1::ToSec1Point;

    use super::super::ctaphid::testing::SimulatedDevice;
    use super::*;

    const CREDENTIAL_ID: &[u8] = b"credential-0001";
    const TOKEN: [u8; 32] = [0x42; 32];

    /// Records prompts and supplies scripted PINs.
    #[derive(Default)]
    struct TestInteraction {
        pins: Mutex<Vec<&'static str>>,
        pin_requests: AtomicU32,
        uv_prompts: AtomicU32,
    }

    impl Interaction for TestInteraction {
        fn prompt_touch(&self) {}

        fn prompt_uv(&self) {
            self.uv_prompts.fetch_add(1, Ordering::SeqCst);
        }

        fn request_pin(&self, attempt: u32) -> Result<Zeroizing<String>> {
            assert_eq!(attempt, self.pin_requests.fetch_add(1, Ordering::SeqCst));
            let pin = self.pins.lock().unwrap().remove(0);
            Ok(Zeroizing::new(pin.to_string()))
        }

        fn choose(&self, _kind: &str, _labels: &[String]) -> Result<usize> {
            Ok(0)
        }
    }

    /// A simulated FIDO2 authenticator.
    struct FakeKey {
        options: Vec<(&'static str, bool)>,
        protocols: Vec<i64>,
        pin: Option<&'static str>,
        agreement_key: SecretKey,
        protocol: Option<PinProtocol>,
        shared_secret: Option<Zeroizing<Vec<u8>>>,
        /// The last getAssertion or makeCredential parameters.
        last_request: Option<CborMap>,
    }

    impl FakeKey {
        fn new(options: Vec<(&'static str, bool)>, protocols: Vec<i64>) -> Self {
            Self {
                options,
                protocols,
                pin: None,
                agreement_key: pin::random_secret_key(),
                protocol: None,
                shared_secret: None,
                last_request: None,
            }
        }

        fn has(&self, option: &str) -> bool {
            self.options
                .iter()
                .any(|&(name, value)| name == option && value)
        }

        fn handle(&mut self, request: &[u8]) -> Vec<u8> {
            let params = match ciborium::de::from_reader::<Value, _>(&request[1..]) {
                Ok(Value::Map(map)) => map,
                _ => Vec::new(),
            };
            match request[0] {
                CMD_GET_INFO => self.reply(Value::Map(vec![
                    (
                        int(1),
                        Value::Array(vec![text("FIDO_2_0"), text("FIDO_2_1")]),
                    ),
                    (int(3), bytes(&[0; 16])),
                    (
                        int(4),
                        Value::Map(
                            self.options
                                .iter()
                                .map(|&(name, value)| (text(name), Value::Bool(value)))
                                .collect(),
                        ),
                    ),
                    (
                        int(6),
                        Value::Array(self.protocols.iter().map(|&p| int(p)).collect()),
                    ),
                ])),
                CMD_CLIENT_PIN => self.client_pin(&params),
                CMD_GET_ASSERTION => {
                    if let Some(status) = self.check_pin_auth(&params, 2, 6) {
                        return vec![status];
                    }
                    self.last_request = Some(params);
                    self.reply(Value::Map(vec![
                        (int(1), credential_descriptor(CREDENTIAL_ID)),
                        (int(2), bytes(b"authenticator-data")),
                        (int(3), bytes(b"signature")),
                    ]))
                }
                CMD_MAKE_CREDENTIAL => {
                    if let Some(status) = self.check_pin_auth(&params, 1, 8) {
                        return vec![status];
                    }
                    self.last_request = Some(params);
                    let mut auth_data = vec![0u8; 32];
                    auth_data.push(0x41); // UP | AT
                    auth_data.extend_from_slice(&[0, 0, 0, 1]);
                    auth_data.extend_from_slice(&[0; 16]);
                    auth_data.extend_from_slice(&(CREDENTIAL_ID.len() as u16).to_be_bytes());
                    auth_data.extend_from_slice(CREDENTIAL_ID);
                    auth_data.extend_from_slice(b"cose-public-key");
                    self.reply(Value::Map(vec![
                        (int(1), text("none")),
                        (int(2), Value::Bytes(auth_data)),
                        (int(3), Value::Map(Vec::new())),
                    ]))
                }
                other => panic!("unexpected CTAP2 command 0x{other:02x}"),
            }
        }

        fn client_pin(&mut self, params: &CborMap) -> Vec<u8> {
            let protocol = match get(params, 1).and_then(Value::as_integer).map(i128::from) {
                Some(2) => PinProtocol::Two,
                _ => PinProtocol::One,
            };
            let subcommand = get(params, 2).and_then(Value::as_integer).map(i128::from);
            match subcommand {
                Some(2) => {
                    let point = self.agreement_key.public_key().to_sec1_point(false);
                    self.reply(Value::Map(vec![(
                        int(1),
                        Value::Map(vec![
                            (int(1), int(2)),
                            (int(3), int(-25)),
                            (int(-1), int(1)),
                            (int(-2), bytes(point.x().unwrap())),
                            (int(-3), bytes(point.y().unwrap())),
                        ]),
                    )]))
                }
                Some(5) | Some(9) => {
                    let scoped = subcommand == Some(9);
                    assert_eq!(scoped, self.has("pinUvAuthToken"));
                    let platform = get(params, 3).and_then(Value::as_map).unwrap();
                    let x = get(platform, -2).and_then(Value::as_bytes).unwrap();
                    let y = get(platform, -3).and_then(Value::as_bytes).unwrap();
                    let shared = pin::shared_secret(protocol, &self.agreement_key, x, y).unwrap();

                    let encrypted = get(params, 6).and_then(Value::as_bytes).unwrap();
                    let pin_hash = protocol.decrypt(&shared, encrypted).unwrap();
                    let expected = Sha256::digest(self.pin.unwrap().as_bytes());
                    if pin_hash[..] != expected[..16] {
                        return vec![ERR_PIN_INVALID];
                    }

                    let token = protocol.encrypt(&shared, &TOKEN).unwrap();
                    self.protocol = Some(protocol);
                    self.shared_secret = Some(shared);
                    self.reply(Value::Map(vec![(int(2), Value::Bytes(token))]))
                }
                other => panic!("unexpected clientPIN subcommand {other:?}"),
            }
        }

        /// Verify `pinUvAuthParam` when a PIN is set; return an error status
        /// if it is missing or wrong.
        fn check_pin_auth(&self, params: &CborMap, hash_key: i64, param_key: i64) -> Option<u8> {
            self.pin?;
            let Some(protocol) = self.protocol else {
                return Some(ERR_PUAT_REQUIRED);
            };
            let hash = get(params, hash_key).and_then(Value::as_bytes).unwrap();
            let Some(param) = get(params, param_key).and_then(Value::as_bytes) else {
                return Some(ERR_PUAT_REQUIRED);
            };
            let expected = protocol.authenticate(&TOKEN, hash).unwrap();
            (*param != expected).then_some(ERR_PIN_AUTH_INVALID)
        }

        fn reply(&self, body: Value) -> Vec<u8> {
            let mut response = vec![0u8];
            ciborium::ser::into_writer(&body, &mut response).unwrap();
            response
        }
    }

    type Handler = Box<dyn FnMut(&[u8]) -> Option<Vec<u8>>>;
    type TestSession<'a> = Session<'a, SimulatedDevice<Handler>>;

    /// Run `f` against `key`, returning its result and the key afterwards.
    fn with_key<R>(
        key: FakeKey,
        interaction: &TestInteraction,
        f: impl FnOnce(&mut TestSession<'_>) -> R,
    ) -> (R, Rc<RefCell<FakeKey>>) {
        let key = Rc::new(RefCell::new(key));
        let handler_key = Rc::clone(&key);
        let handler: Handler =
            Box::new(move |request| Some(handler_key.borrow_mut().handle(request)));
        let mut channel = Channel::open(SimulatedDevice::new(handler)).unwrap();
        let mut session = Session::new(&mut channel, interaction).unwrap();
        (f(&mut session), key)
    }

    fn assertion_request(user_verification: UserVerification) -> AssertionRequest {
        AssertionRequest {
            rp_id: "launchpad.net".to_string(),
            client_data_hash: [0x11; 32],
            allow_credentials: vec![CREDENTIAL_ID.to_vec()],
            user_verification,
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn assertion_without_user_verification() {
        let interaction = TestInteraction::default();
        let key = FakeKey::new(vec![("rk", true), ("up", true)], vec![]);
        let (result, key) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Preferred))
        });

        let assertion = result.unwrap();
        assert_eq!(assertion.credential_id, CREDENTIAL_ID);
        assert_eq!(assertion.authenticator_data, b"authenticator-data");
        assert_eq!(assertion.signature, b"signature");
        assert_eq!(assertion.user_handle, None);

        let request = key.borrow().last_request.clone().unwrap();
        assert_eq!(
            get(&request, 1).and_then(Value::as_text),
            Some("launchpad.net")
        );
        assert_eq!(
            get(&request, 2).and_then(Value::as_bytes),
            Some(&vec![0x11; 32])
        );
        assert!(
            get(&request, 6).is_none(),
            "no pinUvAuthParam without a PIN"
        );
        assert_eq!(interaction.pin_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn assertion_with_scoped_pin_token_protocol_two() {
        let interaction = TestInteraction {
            pins: Mutex::new(vec!["123456"]),
            ..Default::default()
        };
        let mut key = FakeKey::new(
            vec![("clientPin", true), ("pinUvAuthToken", true)],
            vec![2, 1],
        );
        key.pin = Some("123456");
        let (result, key) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Preferred))
        });

        assert!(result.is_ok(), "{:?}", result.err());
        assert_eq!(key.borrow().protocol, Some(PinProtocol::Two));
        assert_eq!(interaction.pin_requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wrong_pin_is_retried_with_protocol_one() {
        let interaction = TestInteraction {
            pins: Mutex::new(vec!["0000", "1234"]),
            ..Default::default()
        };
        let mut key = FakeKey::new(vec![("clientPin", true)], vec![1]);
        key.pin = Some("1234");
        let (result, key) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Required))
        });

        assert!(result.is_ok(), "{:?}", result.err());
        assert_eq!(key.borrow().protocol, Some(PinProtocol::One));
        assert_eq!(interaction.pin_requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn discouraged_verification_skips_the_pin() {
        let interaction = TestInteraction::default();
        let key = FakeKey::new(vec![("clientPin", true)], vec![1]);
        let (result, _) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Discouraged))
        });
        assert!(result.is_ok());
        assert_eq!(interaction.pin_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn required_verification_needs_configuration() {
        let interaction = TestInteraction::default();
        let key = FakeKey::new(vec![("clientPin", false)], vec![1]);
        let (result, _) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Required))
        });
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Set a PIN"), "{err}");
    }

    #[test]
    fn builtin_verification_uses_the_uv_option() {
        let interaction = TestInteraction::default();
        let key = FakeKey::new(vec![("uv", true)], vec![]);
        let (result, key) = with_key(key, &interaction, |session| {
            session.get_assertion(&assertion_request(UserVerification::Required))
        });
        assert!(result.is_ok());
        let request = key.borrow().last_request.clone().unwrap();
        let options = get(&request, 5).and_then(Value::as_map).unwrap();
        assert_eq!(get_text(options, "uv"), Some(&Value::Bool(true)));
        assert_eq!(interaction.uv_prompts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn make_credential_builds_attestation_object() {
        let interaction = TestInteraction {
            pins: Mutex::new(vec!["1234"]),
            ..Default::default()
        };
        let mut key = FakeKey::new(vec![("clientPin", true), ("rk", true)], vec![1]);
        key.pin = Some("1234");
        let request = RegistrationRequest {
            rp_id: "launchpad.net".to_string(),
            rp_name: "Launchpad".to_string(),
            user_id: b"user-handle".to_vec(),
            user_name: "example-user".to_string(),
            user_display_name: "Example User".to_string(),
            client_data_hash: [0x22; 32],
            algorithms: vec![-7, -8],
            exclude_credentials: vec![b"old".to_vec()],
            resident_key: ResidentKey::Discouraged,
            user_verification: UserVerification::Discouraged,
            timeout: Duration::from_secs(1),
        };
        let (result, key) = with_key(key, &interaction, |session| {
            session.make_credential(&request)
        });

        let registration = result.unwrap();
        assert_eq!(registration.credential_id, CREDENTIAL_ID);
        // A PIN is used even though verification was discouraged, because
        // CTAP 2.0 keys with a PIN refuse makeCredential without one.
        assert_eq!(interaction.pin_requests.load(Ordering::SeqCst), 1);

        let attestation: Value =
            ciborium::de::from_reader(registration.attestation_object.as_slice()).unwrap();
        let attestation = attestation.as_map().unwrap();
        let keys: Vec<_> = attestation
            .iter()
            .map(|(k, _)| k.as_text().unwrap())
            .collect();
        assert_eq!(keys, ["fmt", "attStmt", "authData"]);

        let sent = key.borrow().last_request.clone().unwrap();
        assert!(get(&sent, 5).is_some(), "exclude list is sent");
        assert!(
            get(&sent, 7).is_none(),
            "no options for a non-resident credential"
        );
    }

    #[test]
    fn status_codes_map_to_errors() {
        assert!(matches!(
            status_error(ERR_KEEPALIVE_CANCEL),
            LpError::HardwareKeyCancelled
        ));
        assert!(matches!(
            status_error(ERR_USER_ACTION_TIMEOUT),
            LpError::HardwareKeyTimeout
        ));
        assert!(
            status_error(ERR_PIN_BLOCKED)
                .to_string()
                .contains("blocked")
        );
        assert!(status_error(0x7F).to_string().contains("0x7f"));
    }

    #[test]
    fn attested_credential_id_requires_the_at_flag() {
        let mut auth_data = vec![0u8; 37];
        assert_eq!(attested_credential_id(&auth_data), None);
        auth_data[32] = 0x40;
        auth_data.extend_from_slice(&[0; 16]);
        auth_data.extend_from_slice(&[0, 3, 9, 8, 7]);
        assert_eq!(attested_credential_id(&auth_data), Some(vec![9, 8, 7]));
        assert_eq!(attested_credential_id(&auth_data[..56]), None);
    }
}
