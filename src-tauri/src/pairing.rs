//! Pairing a Mac running Drift Host from the Connections page (task **IOS-9**; ADR
//! `IOS-9-pairing`).
//!
//! [`PairingService`] turns what the user gave — a scanned or pasted QR payload, or a Bonjour
//! [`HostAdvert`] plus the typed code — into a paired profile: pinned to the host's certificate
//! from the start, marked with the host's id, its account name and optional password saved like
//! any profile's. The host's pin and the device key go to a [`PairingStore`] (the Keychain in
//! the app), and [`PairingService::check_before_connect`] refuses a paired profile whose pin no
//! longer matches what the pairing recorded.
//!
//! The handshake that lets the Mac check the code and store the device key runs over the
//! `Drift::Mac::v1` channel on the next connection (HOST-8, ADR `HOST-8-pairing`):
//! [`PairingService::client_pairing`] gives that connection the code the user saw, once; after
//! the Mac accepted it, every connection proves the device key instead. The Mac's account
//! password still signs the session in (NLA) as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use drift_core::CertFingerprint;
use drift_core::pairing::{HostAdvert, PairingPayload, PairingTarget, ShortCode};
use drift_ext::pairing::{ClientPairing, DeviceSigner, PairingOutcome};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profiles::{CommandError, LinuxPasswordUpdate, ProfileEntry, ProfileService, SecretsUpdate};

/// Where the device key and the paired hosts' pins live.
pub trait PairingStore: Send + Sync {
    /// The device's public key, creating the key pair on first use.
    fn device_public_key(&self) -> Result<Vec<u8>, CommandError>;
    /// Records host `host`'s certificate fingerprint.
    fn pin_host(&self, host: Uuid, fingerprint: CertFingerprint) -> Result<(), CommandError>;
    /// Host `host`'s recorded fingerprint.
    fn host_pin(&self, host: Uuid) -> Result<Option<CertFingerprint>, CommandError>;
    /// Forgets host `host`.
    fn forget_host(&self, host: Uuid) -> Result<(), CommandError>;
    /// The device key, to sign the host's challenges (HOST-8).
    fn signer(&self) -> Result<Arc<dyn DeviceSigner>, CommandError>;
}

fn storage(e: impl std::fmt::Display) -> CommandError {
    CommandError::Storage { message: e.to_string() }
}

fn pairing(e: impl std::fmt::Display) -> CommandError {
    CommandError::Pairing { message: e.to_string() }
}

/// The production [`PairingStore`]: items in the Keychain (`drift_apple::pairing`).
#[derive(Debug, Clone)]
pub struct KeychainPairingStore(drift_apple::pairing::PairingKeychain);

impl KeychainPairingStore {
    /// Pairing items in `keychain`'s service.
    pub fn new(keychain: drift_apple::Keychain) -> Self {
        Self(drift_apple::pairing::PairingKeychain::new(keychain))
    }
}

impl PairingStore for KeychainPairingStore {
    fn device_public_key(&self) -> Result<Vec<u8>, CommandError> {
        self.0.device_key().map(|k| k.public_key().to_vec()).map_err(storage)
    }

    fn pin_host(&self, host: Uuid, fingerprint: CertFingerprint) -> Result<(), CommandError> {
        self.0.pin_host(host, fingerprint).map_err(storage)
    }

    fn host_pin(&self, host: Uuid) -> Result<Option<CertFingerprint>, CommandError> {
        self.0.host_pin(host).map_err(storage)
    }

    fn forget_host(&self, host: Uuid) -> Result<(), CommandError> {
        self.0.forget_host(host).map_err(storage)
    }

    fn signer(&self) -> Result<Arc<dyn DeviceSigner>, CommandError> {
        Ok(Arc::new(self.0.device_key().map_err(storage)?))
    }
}

/// An in-memory [`PairingStore`] (tests, smoke runs): a real P-256 key that lives as long as the
/// store.
#[derive(Debug, Default)]
pub struct MemoryPairingStore {
    device_key: Mutex<Option<Arc<drift_apple::pairing::DeviceKey>>>,
    pins: Mutex<HashMap<Uuid, CertFingerprint>>,
}

impl MemoryPairingStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl PairingStore for MemoryPairingStore {
    fn device_public_key(&self) -> Result<Vec<u8>, CommandError> {
        Ok(self.memory_key()?.public_key().to_vec())
    }

    fn pin_host(&self, host: Uuid, fingerprint: CertFingerprint) -> Result<(), CommandError> {
        self.pins.lock().map_err(storage)?.insert(host, fingerprint);
        Ok(())
    }

    fn host_pin(&self, host: Uuid) -> Result<Option<CertFingerprint>, CommandError> {
        Ok(self.pins.lock().map_err(storage)?.get(&host).copied())
    }

    fn forget_host(&self, host: Uuid) -> Result<(), CommandError> {
        self.pins.lock().map_err(storage)?.remove(&host);
        Ok(())
    }

    fn signer(&self) -> Result<Arc<dyn DeviceSigner>, CommandError> {
        Ok(self.memory_key()?)
    }
}

impl MemoryPairingStore {
    fn memory_key(&self) -> Result<Arc<drift_apple::pairing::DeviceKey>, CommandError> {
        let mut key = self.device_key.lock().map_err(storage)?;
        if let Some(key) = key.as_ref() {
            return Ok(Arc::clone(key));
        }
        let made = Arc::new(drift_apple::pairing::DeviceKey::generate().map_err(storage)?);
        *key = Some(Arc::clone(&made));
        Ok(made)
    }
}

/// What a scanned, pasted or typed pairing code turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PairingInput {
    /// A QR payload: it names the host, so no Bonjour host needs choosing.
    Qr {
        /// The payload, to pass back to [`PairHostRequest::Qr`].
        uri: String,
        /// The Mac's name.
        name: String,
        /// Host name.
        host: String,
        /// RDP port.
        port: u16,
    },
    /// A typed code, formatted `1234 5678`.
    Code {
        /// The code.
        code: String,
    },
}

/// A pairing to perform.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PairHostRequest {
    /// From a QR payload.
    Qr {
        /// The payload.
        uri: String,
        /// The account on the Mac.
        username: String,
        /// Its password, to store like any profile's (optional).
        password: Option<String>,
    },
    /// A host found with Bonjour and the code typed from its screen.
    Advert {
        /// The host.
        advert: HostAdvert,
        /// The typed code.
        code: String,
        /// The account on the Mac.
        username: String,
        /// Its password (optional).
        password: Option<String>,
    },
    /// A host found with Bonjour that approves devices itself: no code; its user clicks Allow
    /// when the device first connects (HOST-15).
    Approve {
        /// The host.
        advert: HostAdvert,
        /// The account on the Mac.
        username: String,
        /// Its password (optional).
        password: Option<String>,
    },
}

impl std::fmt::Debug for PairHostRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Qr { username, .. } => {
                f.debug_struct("Qr").field("username", username).finish_non_exhaustive()
            }
            Self::Advert { advert, username, .. } => f
                .debug_struct("Advert")
                .field("host", &advert.host)
                .field("username", username)
                .finish_non_exhaustive(),
            Self::Approve { advert, username, .. } => f
                .debug_struct("Approve")
                .field("host", &advert.host)
                .field("username", username)
                .finish_non_exhaustive(),
        }
    }
}

/// How a device introduces itself to a Mac the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Introduction {
    /// The code the user saw on the Mac.
    Code(ShortCode),
    /// Ask the Mac's user to click Allow (HOST-15).
    Approval,
}

/// What the next connections to one paired host do: introduce the device (until the Mac
/// accepts it), then authenticate with the device key.
#[derive(Debug, Default)]
struct HostHandshake {
    introduction: Option<Introduction>,
    current: Option<ClientPairing>,
}

/// Whether `pairing` still introduces the device (a code or a request for approval).
fn introduces(pairing: &ClientPairing) -> bool {
    pairing.code().is_some() || pairing.asks_approval()
}

/// The host a pairing is for.
struct PairedMac {
    host_id: Uuid,
    name: String,
    host: String,
    port: u16,
    fingerprint: CertFingerprint,
}

impl From<PairingTarget> for PairedMac {
    fn from(t: PairingTarget) -> Self {
        Self { host_id: t.host_id, name: t.name, host: t.host, port: t.port, fingerprint: t.fingerprint }
    }
}

impl From<HostAdvert> for PairedMac {
    fn from(a: HostAdvert) -> Self {
        Self { host_id: a.host_id, name: a.name, host: a.host, port: a.port, fingerprint: a.fingerprint }
    }
}

/// Pairs Mac hosts and guards paired profiles. Clones share their state.
#[derive(Clone)]
pub struct PairingService {
    profiles: Arc<ProfileService>,
    store: Arc<dyn PairingStore>,
    handshakes: Arc<Mutex<HashMap<Uuid, HostHandshake>>>,
}

impl std::fmt::Debug for PairingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingService").finish_non_exhaustive()
    }
}

impl PairingService {
    /// A service saving profiles to `profiles` and pairing items to `store`.
    pub fn new(profiles: Arc<ProfileService>, store: Arc<dyn PairingStore>) -> Self {
        Self { profiles, store, handshakes: Arc::default() }
    }

    /// Recognises a QR payload (`drift-pair:…`) or a typed code.
    pub fn read_code(text: &str) -> Result<PairingInput, CommandError> {
        let text = text.trim();
        if text.starts_with("drift-pair:") {
            let target = PairingPayload::parse(text).map_err(pairing)?.target();
            return Ok(PairingInput::Qr {
                uri: text.to_owned(),
                name: target.name,
                host: target.host,
                port: target.port,
            });
        }
        let code = ShortCode::parse(text).map_err(pairing)?;
        Ok(PairingInput::Code { code: code.to_string() })
    }

    /// Pairs: saves (or updates) the host's profile, records its pin and makes sure the device
    /// key exists.
    pub fn pair(&self, request: PairHostRequest) -> Result<ProfileEntry, CommandError> {
        let (target, introduction, username, password): (PairedMac, _, _, _) = match request {
            PairHostRequest::Qr { uri, username, password } => {
                let target = PairingPayload::parse(&uri).map_err(pairing)?.target();
                let code = target.code.clone();
                (target.into(), Introduction::Code(code), username, password)
            }
            PairHostRequest::Advert { advert, code, username, password } => {
                let target = advert.target(ShortCode::parse(&code).map_err(pairing)?);
                let code = target.code.clone();
                (target.into(), Introduction::Code(code), username, password)
            }
            PairHostRequest::Approve { advert, username, password } => {
                if !advert.approval {
                    return Err(CommandError::Pairing {
                        message: "This Mac's Drift Host cannot approve devices. Enter the code it shows."
                            .into(),
                    });
                }
                (advert.into(), Introduction::Approval, username, password)
            }
        };
        let profile = self.paired_profile(&target, username.trim())?;
        // The device key must exist before a host can be told about it (HOST-8).
        self.store.device_public_key()?;
        let update = SecretsUpdate {
            rdp_password: password.filter(|p| !p.is_empty()),
            linux_password: LinuxPasswordUpdate::Keep,
        };
        let entry = self.profiles.save(profile, update)?;
        self.store.pin_host(target.host_id, target.fingerprint)?;
        // The next connection sends the code to the Mac (HOST-8) or asks its user (HOST-15).
        self.handshakes
            .lock()
            .map_err(storage)?
            .insert(target.host_id, HostHandshake { introduction: Some(introduction), current: None });
        Ok(entry)
    }

    /// The pairing exchange for the next connection of `profile`: `None` for a profile that is
    /// not a paired Mac. It carries the code until the Mac accepted it once, and from then on
    /// authenticates with the device key. The returned value is shared with the session, so the
    /// Mac's answer is seen here too.
    pub fn client_pairing(&self, profile: Uuid) -> Result<Option<ClientPairing>, CommandError> {
        let profile = self.profiles.get(profile)?.profile;
        let Some(host) = profile.paired_host else { return Ok(None) };
        let Some(pin) = profile.cert_pin else {
            return Err(CommandError::Pairing {
                message: "This Mac's pairing has no certificate. Pair it again.".into(),
            });
        };
        let mut handshakes = self.handshakes.lock().map_err(storage)?;
        let entry = handshakes.entry(host).or_default();
        if let Some(current) = &entry.current {
            if matches!(current.outcome(), Some(PairingOutcome::Paired | PairingOutcome::Authenticated)) {
                entry.introduction = None;
            } else if current.outcome().is_none() && introduces(current) == entry.introduction.is_some() {
                // Still in progress (a reconnect within the session): keep the same exchange.
                return Ok(Some(current.clone()));
            }
        }
        let base = ClientPairing::new(
            self.store.signer()?,
            *pin.as_bytes(),
            drift_rdp::connect::local_client_name(),
        );
        let pairing = match &entry.introduction {
            Some(Introduction::Code(code)) => base.with_code(code.as_str()),
            Some(Introduction::Approval) => base.asking_approval(),
            None => base,
        };
        entry.current = Some(pairing.clone());
        Ok(Some(pairing))
    }

    /// The new profile, or the existing one for this host (same id and preferences) updated.
    fn paired_profile(
        &self,
        target: &PairedMac,
        username: &str,
    ) -> Result<drift_core::ConnectionProfile, CommandError> {
        let existing = self.profiles.profiles()?.into_iter().find(|p| p.paired_host == Some(target.host_id));
        let mut profile = existing.unwrap_or_else(|| {
            drift_core::ConnectionProfile::new(
                target.name.clone(),
                target.host.clone(),
                drift_core::ConnectMode::Headless,
            )
        });
        profile.host.clone_from(&target.host);
        profile.port = target.port;
        profile.cert_pin = Some(target.fingerprint);
        profile.paired_host = Some(target.host_id);
        username.clone_into(&mut profile.rdp_username);
        Ok(profile)
    }

    /// Refuses to connect a paired profile whose pin differs from the one the pairing recorded
    /// (or whose pairing record is gone). Other profiles pass.
    pub fn check_before_connect(&self, profile: Uuid) -> Result<(), CommandError> {
        let profile = self.profiles.get(profile)?.profile;
        let Some(host) = profile.paired_host else { return Ok(()) };
        match self.store.host_pin(host)? {
            Some(pin) if Some(pin) == profile.cert_pin => Ok(()),
            _ => Err(CommandError::Pairing {
                message: "This Mac's pairing record is missing or does not match. Pair it again.".into(),
            }),
        }
    }
}

/// The first `drift-pair:` link among URLs the system opened Drift with (the Camera app scanning
/// the Mac's QR code, or a click on a Mac).
pub fn pairing_link<'a>(mut urls: impl Iterator<Item = &'a str>) -> Option<String> {
    urls.find(|u| u.starts_with("drift-pair:")).map(str::to_owned)
}

/// Emitted to the `connections` page while discovery runs: every Drift Host currently found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct HostsDiscovered(pub Vec<HostAdvert>);
