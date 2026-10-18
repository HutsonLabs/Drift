//! Pairing keys in the Keychain (task **IOS-9**, ADR `IOS-9-pairing`).
//!
//! The device has one P-256 key pair, created on first use and kept as a Keychain item; its
//! public key goes to the Mac when pairing (HOST-8 stores it) and later proves the device is the
//! one that paired. Each paired host's certificate fingerprint is kept as an item too, so a
//! profile's pin can be checked against what the pairing recorded before connecting.
//!
//! Items live in Drift's Keychain service next to the passwords, under `pairing/…` accounts,
//! which can never be a password account (`<uuid>/<role>`). The key is a software key: Secure
//! Enclave keys cannot be exported to an item, are device-only and do not exist in the
//! simulator; moving to one is a later hardening step that keeps this interface.

use drift_core::CertFingerprint;
use ring::rand::SystemRandom;
use ring::signature::{self, EcdsaKeyPair, KeyPair as _};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::keychain::{Keychain, KeychainError};

/// The account of the device key (PKCS#8 document).
pub const DEVICE_KEY_ACCOUNT: &str = "pairing/device-key";

/// The account of host `host`'s pinned certificate fingerprint.
pub fn host_pin_account(host: Uuid) -> String {
    format!("pairing/host/{host}")
}

/// The device's pairing key pair.
pub struct DeviceKey {
    pair: EcdsaKeyPair,
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKey").field("public_key_len", &self.public_key().len()).finish_non_exhaustive()
    }
}

impl DeviceKey {
    /// A new key held only in memory (tests, and the host's own checks); the app's key comes from
    /// [`PairingKeychain::device_key`].
    pub fn generate() -> Result<Self, KeychainError> {
        let document =
            EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .map_err(|_| KeychainError { code: 0, message: "could not generate a device key".into() })?;
        Self::from_pkcs8(document.as_ref())
    }

    fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, KeychainError> {
        let rng = SystemRandom::new();
        EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng)
            .map(|pair| Self { pair })
            .map_err(|_| KeychainError {
                code: 0,
                message: "the stored device key is not a P-256 key".into(),
            })
    }

    /// The public key, an uncompressed P-256 point (65 bytes, `04 ‖ x ‖ y`).
    pub fn public_key(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    /// An ECDSA P-256 SHA-256 signature (ASN.1 DER) of `message`.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>, KeychainError> {
        self.pair
            .sign(&SystemRandom::new(), message)
            .map(|s| s.as_ref().to_vec())
            .map_err(|_| KeychainError { code: 0, message: "signing failed".into() })
    }
}

impl drift_ext::pairing::DeviceSigner for DeviceKey {
    fn public_key(&self) -> Vec<u8> {
        DeviceKey::public_key(self).to_vec()
    }

    fn sign(&self, message: &[u8]) -> Option<Vec<u8>> {
        DeviceKey::sign(self, message).ok()
    }
}

/// Whether `signature` is a valid [`DeviceKey::sign`] signature of `message` by `public_key`.
pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, public_key)
        .verify(message, signature)
        .is_ok()
}

/// The device key and host pins in a [`Keychain`].
#[derive(Debug, Clone)]
pub struct PairingKeychain {
    keychain: Keychain,
}

impl PairingKeychain {
    /// Pairing items in `keychain`'s service.
    pub fn new(keychain: Keychain) -> Self {
        Self { keychain }
    }

    /// The device key, created and stored on first use.
    pub fn device_key(&self) -> Result<DeviceKey, KeychainError> {
        if let Some(pkcs8) = self.keychain.get_item(DEVICE_KEY_ACCOUNT)? {
            return DeviceKey::from_pkcs8(&pkcs8);
        }
        let document =
            EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .map_err(|_| KeychainError {
                    code: 0,
                    message: "could not generate the device key".into(),
                })?;
        let pkcs8 = Zeroizing::new(document.as_ref().to_vec());
        self.keychain.set_item(DEVICE_KEY_ACCOUNT, &pkcs8)?;
        DeviceKey::from_pkcs8(&pkcs8)
    }

    /// Records host `host`'s certificate fingerprint.
    pub fn pin_host(&self, host: Uuid, fingerprint: CertFingerprint) -> Result<(), KeychainError> {
        self.keychain.set_item(&host_pin_account(host), fingerprint.as_bytes())
    }

    /// Host `host`'s recorded fingerprint, if it is paired.
    pub fn host_pin(&self, host: Uuid) -> Result<Option<CertFingerprint>, KeychainError> {
        let Some(bytes) = self.keychain.get_item(&host_pin_account(host))? else { return Ok(None) };
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| KeychainError { code: 0, message: "a stored host pin is not 32 bytes".into() })?;
        Ok(Some(CertFingerprint::from_bytes(bytes)))
    }

    /// Forgets host `host` (unpairing); a missing item is fine.
    pub fn forget_host(&self, host: Uuid) -> Result<(), KeychainError> {
        self.keychain.delete_item(&host_pin_account(host))
    }
}
