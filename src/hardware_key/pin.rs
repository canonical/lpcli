//! CTAP2 PIN/UV auth protocols one and two.
//!
//! Before a key releases a `pinUvAuthToken`, the platform and the key agree
//! on a shared secret with ephemeral ECDH over P-256.  The PIN hash and the
//! token are then exchanged encrypted with AES-256-CBC under that secret,
//! and each later command is authenticated with HMAC-SHA-256 keyed by the
//! token.
//!
//! The two protocols differ only in their key derivation, IV handling and
//! MAC length:
//!
//! | | Protocol one (CTAP 2.0) | Protocol two (CTAP 2.1) |
//! |---|---|---|
//! | KDF | `SHA-256(Z)` | HKDF-SHA-256 → HMAC key ‖ AES key |
//! | IV | all zeroes | random, prepended to the ciphertext |
//! | MAC | first 16 bytes of HMAC | full 32-byte HMAC |
//!
//! See section 6.5, "authenticatorClientPIN", of the CTAP 2.1 specification.

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::{FromSec1Point, ToSec1Point};
use p256::{FieldBytes, PublicKey, Sec1Point, SecretKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{LpError, Result};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type HmacSha256 = Hmac<Sha256>;

const AES_BLOCK_SIZE: usize = 16;
const COORDINATE_SIZE: usize = 32;

/// A CTAP2 PIN/UV auth protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinProtocol {
    One,
    Two,
}

impl PinProtocol {
    /// The protocol number used on the wire.
    pub(crate) fn version(self) -> i64 {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }

    /// Choose the best protocol the key supports.  CTAP 2.0 keys may omit
    /// the list, in which case protocol one is implied.
    pub(crate) fn select(supported: &[u64]) -> Option<Self> {
        if supported.contains(&2) {
            Some(Self::Two)
        } else if supported.is_empty() || supported.contains(&1) {
            Some(Self::One)
        } else {
            None
        }
    }

    /// Derive the shared secret from the ECDH x-coordinate `z`.
    fn kdf(self, z: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        match self {
            Self::One => Ok(Zeroizing::new(Sha256::digest(z).to_vec())),
            Self::Two => {
                let hkdf = hkdf::Hkdf::<Sha256>::new(Some(&[0u8; 32]), z);
                let mut secret = Zeroizing::new(vec![0u8; 64]);
                hkdf.expand(b"CTAP2 HMAC key", &mut secret[..32])
                    .and_then(|()| hkdf.expand(b"CTAP2 AES key", &mut secret[32..]))
                    .map_err(|_| crypto_error())?;
                Ok(secret)
            }
        }
    }

    /// The AES key within a shared secret.
    fn aes_key(self, shared_secret: &[u8]) -> Result<&[u8]> {
        let key = match self {
            Self::One => shared_secret.get(..32),
            Self::Two => shared_secret.get(32..64),
        };
        key.ok_or_else(crypto_error)
    }

    /// Encrypt a block-aligned `plaintext` under `shared_secret`.
    pub(crate) fn encrypt(self, shared_secret: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        if !plaintext.len().is_multiple_of(AES_BLOCK_SIZE) {
            return Err(crypto_error());
        }
        let key = self.aes_key(shared_secret)?;
        let iv: [u8; AES_BLOCK_SIZE] = match self {
            Self::One => [0; AES_BLOCK_SIZE],
            Self::Two => rand::random(),
        };

        let mut buffer = plaintext.to_vec();
        Aes256CbcEnc::new_from_slices(key, &iv)
            .map_err(|_| crypto_error())?
            .encrypt_padded_mut::<NoPadding>(&mut buffer, plaintext.len())
            .map_err(|_| crypto_error())?;

        Ok(match self {
            Self::One => buffer,
            Self::Two => [iv.as_slice(), &buffer].concat(),
        })
    }

    /// Decrypt `ciphertext` produced by the key under `shared_secret`.
    pub(crate) fn decrypt(
        self,
        shared_secret: &[u8],
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        let key = self.aes_key(shared_secret)?;
        let (iv, body) = match self {
            Self::One => ([0; AES_BLOCK_SIZE].as_slice(), ciphertext),
            Self::Two => ciphertext
                .split_at_checked(AES_BLOCK_SIZE)
                .ok_or_else(crypto_error)?,
        };
        if body.is_empty() || !body.len().is_multiple_of(AES_BLOCK_SIZE) {
            return Err(crypto_error());
        }

        let mut buffer = Zeroizing::new(body.to_vec());
        Aes256CbcDec::new_from_slices(key, iv)
            .map_err(|_| crypto_error())?
            .decrypt_padded_mut::<NoPadding>(&mut buffer)
            .map_err(|_| crypto_error())?;
        Ok(buffer)
    }

    /// Compute the `pinUvAuthParam` for `message`, keyed by a
    /// `pinUvAuthToken` (or by a shared secret).
    pub(crate) fn authenticate(self, key: &[u8], message: &[u8]) -> Result<Vec<u8>> {
        let key = match self {
            Self::One => key,
            // Protocol two keys with the HMAC half of a shared secret, which
            // for a 32-byte token is the whole token.
            Self::Two => key.get(..32).ok_or_else(crypto_error)?,
        };
        let mut mac = HmacSha256::new_from_slice(key).map_err(|_| crypto_error())?;
        mac.update(message);
        let tag = mac.finalize().into_bytes();
        Ok(match self {
            Self::One => tag[..16].to_vec(),
            Self::Two => tag.to_vec(),
        })
    }
}

/// The platform half of a key agreement.
pub(crate) struct KeyAgreement {
    /// The x-coordinate of the platform's ephemeral public key.
    pub(crate) public_x: Vec<u8>,
    /// The y-coordinate of the platform's ephemeral public key.
    pub(crate) public_y: Vec<u8>,
    /// The derived shared secret.
    pub(crate) shared_secret: Zeroizing<Vec<u8>>,
}

/// Agree on a shared secret with the key's public key `(peer_x, peer_y)`
/// using a fresh ephemeral platform key.
pub(crate) fn key_agreement(
    protocol: PinProtocol,
    peer_x: &[u8],
    peer_y: &[u8],
) -> Result<KeyAgreement> {
    let secret = random_secret_key();
    let shared_secret = shared_secret(protocol, &secret, peer_x, peer_y)?;

    let public = secret.public_key().to_sec1_point(false);
    let (Some(x), Some(y)) = (public.x(), public.y()) else {
        return Err(crypto_error());
    };
    Ok(KeyAgreement {
        public_x: x.to_vec(),
        public_y: y.to_vec(),
        shared_secret,
    })
}

/// Derive the shared secret between `secret` and the public key
/// `(peer_x, peer_y)`.
pub(crate) fn shared_secret(
    protocol: PinProtocol,
    secret: &SecretKey,
    peer_x: &[u8],
    peer_y: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if peer_x.len() != COORDINATE_SIZE || peer_y.len() != COORDINATE_SIZE {
        return Err(crypto_error());
    }
    let x = FieldBytes::try_from(peer_x).map_err(|_| crypto_error())?;
    let y = FieldBytes::try_from(peer_y).map_err(|_| crypto_error())?;
    let point = Sec1Point::from_affine_coordinates(&x, &y, false);
    let peer = Option::<PublicKey>::from(PublicKey::from_sec1_point(&point)).ok_or_else(|| {
        LpError::HardwareKey("The security key sent an invalid public key.".into())
    })?;

    let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine());
    protocol.kdf(shared.raw_secret_bytes())
}

/// Generate a random P-256 private key.
pub(crate) fn random_secret_key() -> SecretKey {
    loop {
        // Fewer than 1 in 2^32 random strings fall outside the scalar
        // field, so this almost never loops.
        let bytes = Zeroizing::new(rand::random::<[u8; 32]>());
        if let Ok(secret) = SecretKey::from_slice(bytes.as_slice()) {
            return secret;
        }
    }
}

fn crypto_error() -> LpError {
    LpError::HardwareKey("PIN protocol cryptography failed.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinates(secret: &SecretKey) -> (Vec<u8>, Vec<u8>) {
        let point = secret.public_key().to_sec1_point(false);
        (point.x().unwrap().to_vec(), point.y().unwrap().to_vec())
    }

    #[test]
    fn select_prefers_protocol_two() {
        assert_eq!(PinProtocol::select(&[1, 2]), Some(PinProtocol::Two));
        assert_eq!(PinProtocol::select(&[2]), Some(PinProtocol::Two));
        assert_eq!(PinProtocol::select(&[1]), Some(PinProtocol::One));
        assert_eq!(PinProtocol::select(&[]), Some(PinProtocol::One));
        assert_eq!(PinProtocol::select(&[7]), None);
    }

    #[test]
    fn both_sides_derive_the_same_secret() {
        for protocol in [PinProtocol::One, PinProtocol::Two] {
            let device = random_secret_key();
            let (device_x, device_y) = coordinates(&device);

            let platform = key_agreement(protocol, &device_x, &device_y).unwrap();
            let device_side =
                shared_secret(protocol, &device, &platform.public_x, &platform.public_y).unwrap();

            assert_eq!(*platform.shared_secret, *device_side);
            let expected_len = if protocol == PinProtocol::One { 32 } else { 64 };
            assert_eq!(platform.shared_secret.len(), expected_len);
        }
    }

    #[test]
    fn protocol_one_uses_a_zero_iv() {
        let secret = [7u8; 32];
        let plaintext = [1u8; 16];
        let first = PinProtocol::One.encrypt(&secret, &plaintext).unwrap();
        let second = PinProtocol::One.encrypt(&secret, &plaintext).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 16);
        assert_eq!(
            *PinProtocol::One.decrypt(&secret, &first).unwrap(),
            plaintext
        );
    }

    #[test]
    fn protocol_two_prepends_a_random_iv() {
        let secret = [9u8; 64];
        let plaintext = [3u8; 32];
        let first = PinProtocol::Two.encrypt(&secret, &plaintext).unwrap();
        let second = PinProtocol::Two.encrypt(&secret, &plaintext).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 16 + 32);
        assert_eq!(
            *PinProtocol::Two.decrypt(&secret, &first).unwrap(),
            plaintext
        );
    }

    #[test]
    fn encryption_rejects_unaligned_input() {
        assert!(PinProtocol::One.encrypt(&[0u8; 32], &[0u8; 15]).is_err());
        assert!(PinProtocol::Two.decrypt(&[0u8; 64], &[0u8; 20]).is_err());
    }

    #[test]
    fn authenticate_truncates_only_for_protocol_one() {
        let token = [5u8; 32];
        assert_eq!(
            PinProtocol::One.authenticate(&token, b"m").unwrap().len(),
            16
        );
        assert_eq!(
            PinProtocol::Two.authenticate(&token, b"m").unwrap().len(),
            32
        );
        // Protocol one's tag is a prefix of the same HMAC.
        assert_eq!(
            PinProtocol::One.authenticate(&token, b"m").unwrap(),
            PinProtocol::Two.authenticate(&token, b"m").unwrap()[..16]
        );
    }

    #[test]
    fn rejects_points_off_the_curve() {
        assert!(shared_secret(PinProtocol::Two, &random_secret_key(), &[1; 32], &[2; 32]).is_err());
        assert!(shared_secret(PinProtocol::Two, &random_secret_key(), &[1; 31], &[2; 32]).is_err());
    }
}
