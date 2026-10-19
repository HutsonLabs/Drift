//! Pairing and device authentication, host side (task **HOST-8**; ADR `HOST-8-pairing`).
//!
//! The Mac shows a code (a [`ShortCode`]: seven random digits and a check digit) for
//! [`CODE_LIFETIME`]. A device that sends it over `Drift::Mac::v1`, with its public key and a
//! signature of the host's challenge, is added to the list of paired devices ([`DeviceStore`],
//! `devices.toml`). Later connections only prove the key ([`AuthStatus`]). Until a connection is
//! authorized the host shows it nothing and ignores its input; [`MAX_WRONG_CODES`] wrong codes
//! lock pairing for [`LOCKOUT`].
//!
//! [`Pairing`] is the host's shared state; [`Pairing::responder`] makes the channel responder of
//! one connection, which reports to that connection's [`ConnectionAuth`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use drift_core::Clock;
use drift_core::pairing::ShortCode;
use drift_ext::Negotiated;
use drift_ext::channel::Responder;
use drift_ext::pairing::{
    AuthStatus, NONCE_LEN, PAIRING_VERSION, PairStatus, PairingMessage, Purpose, signed_bytes,
};
use drift_ext::wire::{Capabilities, Message};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How long a code on the Mac's screen stays valid.
pub const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Wrong codes in a row before pairing locks.
pub const MAX_WRONG_CODES: u32 = 5;

/// How long pairing stays locked after too many wrong codes.
pub const LOCKOUT: Duration = Duration::from_secs(60);

/// How long a connection may take to authenticate before the host drops it.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

/// File name of the device list in the host's config directory.
pub const DEVICES_FILE: &str = "devices.toml";

/// How long a device that asked to be approved waits for the Mac's user (HOST-15).
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Sends a channel message to the connection's device outside a reply: the answer to an
/// approval request, once the Mac's user decided (HOST-15).
pub type ReplySender = Arc<dyn Fn(Message) + Send + Sync>;

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the device list ----------------------------------------------------------------------------

/// A device that paired with this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Stable id (to remove it).
    pub id: Uuid,
    /// The name the device gave when pairing.
    pub name: String,
    /// Its public key, hex (uncompressed P-256 point).
    pub key: String,
    /// When it paired, seconds since the Unix epoch.
    pub paired_at: u64,
}

impl PairedDevice {
    /// The public key's bytes.
    pub fn public_key(&self) -> Vec<u8> {
        (0..self.key.len() / 2)
            .filter_map(|i| self.key.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .collect()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default, rename = "device")]
    devices: Vec<PairedDevice>,
}

/// Reading or writing `devices.toml` failed.
#[derive(Debug, thiserror::Error)]
pub enum DeviceStoreError {
    /// The file could not be read or written.
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The file is not a device list.
    #[error("{0} is not a device list: {1}")]
    Parse(PathBuf, String),
}

/// Writes a whole `devices.toml` somewhere else: through the daemon, which owns the file when
/// the host runs as a daemon and agents (HOST-13).
pub type DeviceSaver = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// The paired devices, kept in `devices.toml` (or only in memory).
#[derive(Clone)]
pub struct DeviceStore {
    path: Option<PathBuf>,
    devices: Vec<PairedDevice>,
    saver: Option<DeviceSaver>,
}

impl std::fmt::Debug for DeviceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceStore")
            .field("path", &self.path)
            .field("devices", &self.devices.len())
            .field("brokered", &self.saver.is_some())
            .finish()
    }
}

/// Parses a `devices.toml` (the daemon checks what an agent asks it to write, HOST-13).
pub fn parse_devices(text: &str) -> Result<Vec<PairedDevice>, String> {
    toml::from_str::<DevicesFile>(text).map(|f| f.devices).map_err(|e| e.to_string())
}

impl DeviceStore {
    /// An empty list that is never saved (tests).
    pub fn memory() -> Self {
        Self { path: None, devices: Vec::new(), saver: None }
    }

    /// The list in `dir/devices.toml`; a missing file is an empty list.
    pub fn load(dir: &Path) -> Result<Self, DeviceStoreError> {
        let path = dir.join(DEVICES_FILE);
        let devices = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<DevicesFile>(&text)
                    .map_err(|e| DeviceStoreError::Parse(path.clone(), e.to_string()))?
                    .devices
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(DeviceStoreError::Io(path, e)),
        };
        Ok(Self { path: Some(path), devices, saver: None })
    }

    /// The devices.
    pub fn devices(&self) -> &[PairedDevice] {
        &self.devices
    }

    /// Saves through `saver` instead of writing the file itself (an agent, HOST-13).
    #[must_use]
    pub fn with_saver(mut self, saver: DeviceSaver) -> Self {
        self.saver = Some(saver);
        self
    }

    fn save(&self) -> Result<(), DeviceStoreError> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(saver) = &self.saver {
            let text = toml::to_string(&DevicesFile { devices: self.devices.clone() })
                .map_err(|e| DeviceStoreError::Parse(path.clone(), e.to_string()))?;
            return saver(&text).map_err(|e| DeviceStoreError::Io(path.clone(), std::io::Error::other(e)));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DeviceStoreError::Io(dir.to_owned(), e))?;
        }
        let text = toml::to_string(&DevicesFile { devices: self.devices.clone() })
            .map_err(|e| DeviceStoreError::Parse(path.clone(), e.to_string()))?;
        std::fs::write(path, text).map_err(|e| DeviceStoreError::Io(path.clone(), e))
    }

    fn add(&mut self, name: &str, key: &[u8]) -> PairedDevice {
        let key = hex(key);
        // Pairing the same key again replaces the old entry (a renamed or re-paired device).
        self.devices.retain(|d| d.key != key);
        let paired_at =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let device = PairedDevice { id: Uuid::new_v4(), name: name.to_owned(), key, paired_at };
        self.devices.push(device.clone());
        if let Err(e) = self.save() {
            tracing::error!(error = %e, "could not save the paired devices");
        }
        device
    }

    fn find(&self, key: &[u8]) -> Option<&PairedDevice> {
        let key = hex(key);
        self.devices.iter().find(|d| d.key == key)
    }

    fn remove(&mut self, id: Uuid) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        let removed = self.devices.len() != before;
        if removed && let Err(e) = self.save() {
            tracing::error!(error = %e, "could not save the paired devices");
        }
        removed
    }
}

// ---- the pairing window -------------------------------------------------------------------------

#[derive(Debug)]
struct Window {
    code: ShortCode,
    opened: Instant,
}

/// A device waiting for the Mac's user to approve it (HOST-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// Answers it ([`Pairing::approve`], [`Pairing::decline`]).
    pub id: u64,
    /// The name the device gave.
    pub device_name: String,
}

struct Asked {
    id: u64,
    device_name: String,
    key: Vec<u8>,
    asked: Instant,
    auth: ConnectionAuth,
    reply: ReplySender,
}

impl std::fmt::Debug for Asked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asked").field("id", &self.id).field("device_name", &self.device_name).finish()
    }
}

#[derive(Debug)]
struct State {
    store: DeviceStore,
    window: Option<Window>,
    wrong_in_a_row: u32,
    locked_until: Option<Instant>,
    asked: Vec<Asked>,
}

impl State {
    /// Whether pairing is locked at `now`; an ended lockout closes the window and starts over.
    fn locked(&mut self, now: Instant) -> bool {
        if let Some(until) = self.locked_until {
            if now < until {
                return true;
            }
            self.locked_until = None;
            self.window = None;
            self.wrong_in_a_row = 0;
        }
        false
    }

    /// Counts a wrong code or a Don't Allow; too many in a row lock pairing. `true` if locked.
    fn count_wrong(&mut self, now: Instant) -> bool {
        self.wrong_in_a_row += 1;
        if self.wrong_in_a_row >= MAX_WRONG_CODES {
            self.locked_until = Some(now + LOCKOUT);
            return true;
        }
        false
    }
}

struct Shared {
    state: Mutex<State>,
    clock: Arc<dyn Clock>,
    wrong_codes: AtomicU64,
    next_request: AtomicU64,
    auth_timeout: Duration,
    approval_timeout: Duration,
}

/// The host's pairing state: the code on screen and the paired devices. Clones share it.
#[derive(Clone)]
pub struct Pairing(Arc<Shared>);

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("devices", &lock(&self.0.state).store.devices().len())
            .field("open", &self.code().is_some())
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// Pairing over `store`, timed by `clock`.
    pub fn new(store: DeviceStore, clock: Arc<dyn Clock>) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                store,
                window: None,
                wrong_in_a_row: 0,
                locked_until: None,
                asked: Vec::new(),
            }),
            clock,
            wrong_codes: AtomicU64::new(0),
            next_request: AtomicU64::new(1),
            auth_timeout: AUTH_TIMEOUT,
            approval_timeout: APPROVAL_TIMEOUT,
        }))
    }

    /// The same pairing with a different authentication deadline (tests).
    #[must_use]
    pub fn with_auth_timeout(mut self, timeout: Duration) -> Self {
        // Only before the pairing is shared (`Pairing::new(…).with_auth_timeout(…)`).
        if let Some(shared) = Arc::get_mut(&mut self.0) {
            shared.auth_timeout = timeout;
        }
        self
    }

    /// The same pairing with a different wait for the Mac's user (tests).
    #[must_use]
    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.0) {
            shared.approval_timeout = timeout;
        }
        self
    }

    /// How long a connection may take to authenticate.
    pub fn auth_timeout(&self) -> Duration {
        self.0.auth_timeout
    }

    /// How long a device waits for the Mac's user to approve it.
    pub fn approval_timeout(&self) -> Duration {
        self.0.approval_timeout
    }

    // ---- approval on the Mac (HOST-15) ----

    /// Asks the Mac's user to approve `key` as `name` for the connection `auth`; `reply` gets the
    /// answer later. `Err` with the status to answer now when pairing is locked.
    fn ask(
        &self,
        name: &str,
        key: &[u8],
        auth: &ConnectionAuth,
        reply: ReplySender,
    ) -> Result<u64, PairStatus> {
        let now = self.0.clock.now();
        let mut state = lock(&self.0.state);
        if state.locked(now) {
            return Err(PairStatus::TooManyAttempts);
        }
        let id = self.0.next_request.fetch_add(1, Ordering::Relaxed);
        state.asked.push(Asked {
            id,
            device_name: name.to_owned(),
            key: key.to_vec(),
            asked: now,
            auth: auth.clone(),
            reply,
        });
        tracing::info!(device = %name, id, "device asks to be approved");
        Ok(id)
    }

    /// The devices waiting for the Mac's user, oldest first (a request older than
    /// [`Pairing::approval_timeout`] is no longer offered).
    pub fn approvals(&self) -> Vec<PendingApproval> {
        let now = self.0.clock.now();
        lock(&self.0.state)
            .asked
            .iter()
            .filter(|a| now.duration_since(a.asked) <= self.0.approval_timeout)
            .map(|a| PendingApproval { id: a.id, device_name: a.device_name.clone() })
            .collect()
    }

    /// Takes request `id` out of the queue; with `fresh`, only while it is still offered.
    fn take_asked(&self, id: u64, fresh: bool) -> Option<Asked> {
        let now = self.0.clock.now();
        let mut state = lock(&self.0.state);
        let i = state
            .asked
            .iter()
            .position(|a| a.id == id && (!fresh || now.duration_since(a.asked) <= self.0.approval_timeout))?;
        Some(state.asked.remove(i))
    }

    /// The Mac's user chose Allow: pairs the device and lets its connection in. `false` if the
    /// request is gone or too old.
    pub fn approve(&self, id: u64) -> bool {
        let Some(asked) = self.take_asked(id, true) else { return false };
        let device = {
            let mut state = lock(&self.0.state);
            state.wrong_in_a_row = 0;
            state.store.add(&asked.device_name, &asked.key)
        };
        tracing::info!(device = %device.name, id = %device.id, "device approved and paired");
        (asked.reply)(PairingMessage::PairReply(PairStatus::Accepted).to_message());
        asked.auth.authorize(device);
        true
    }

    /// The Mac's user chose Don't Allow; repeated refusals lock pairing like wrong codes.
    pub fn decline(&self, id: u64) -> bool {
        let Some(asked) = self.take_asked(id, false) else { return false };
        lock(&self.0.state).count_wrong(self.0.clock.now());
        tracing::info!(device = %asked.device_name, "device declined");
        (asked.reply)(PairingMessage::PairReply(PairStatus::Declined).to_message());
        asked.auth.refuse();
        true
    }

    /// Nobody answered in time: tells the device and refuses it.
    pub fn give_up(&self, id: u64) -> bool {
        let Some(asked) = self.take_asked(id, false) else { return false };
        tracing::info!(device = %asked.device_name, "nobody approved the device in time");
        (asked.reply)(PairingMessage::PairReply(PairStatus::NoAnswer).to_message());
        asked.auth.refuse();
        true
    }

    /// Shows a new random code (replacing any code on screen).
    pub fn open_window(&self) -> ShortCode {
        let mut bytes = [0_u8; 4];
        let body = match ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes) {
            Ok(()) => u32::from_le_bytes(bytes) % 10_000_000,
            // The system RNG does not fail on macOS; the clock is a weak but harmless fallback.
            Err(_) => self.0.clock.now().elapsed().subsec_nanos() % 10_000_000,
        };
        // `from_body` only refuses bodies above 9 999 999, which `% 10_000_000` rules out.
        let code = ShortCode::from_body(body).expect("a seven-digit body");
        self.open_window_with(code)
    }

    /// Shows `code` (tests, and a code the menu-bar app chose).
    pub fn open_window_with(&self, code: ShortCode) -> ShortCode {
        let mut state = lock(&self.0.state);
        state.window = Some(Window { code: code.clone(), opened: self.0.clock.now() });
        code
    }

    /// Hides the code.
    pub fn close_window(&self) {
        lock(&self.0.state).window = None;
    }

    /// The code on screen, while it is valid.
    pub fn code(&self) -> Option<ShortCode> {
        let now = self.0.clock.now();
        let state = lock(&self.0.state);
        state
            .window
            .as_ref()
            .filter(|w| now.duration_since(w.opened) <= CODE_LIFETIME)
            .map(|w| w.code.clone())
    }

    /// Checks `code` and, if it is the one on screen, pairs `key` as `name`.
    pub fn try_pair(&self, code: &str, name: &str, key: &[u8]) -> PairStatus {
        let now = self.0.clock.now();
        let mut state = lock(&self.0.state);
        if state.locked(now) {
            return PairStatus::TooManyAttempts;
        }
        let Some(window) = &state.window else { return PairStatus::NotPairing };
        if now.duration_since(window.opened) > CODE_LIFETIME {
            state.window = None;
            return PairStatus::Expired;
        }
        if !constant_time_eq(window.code.as_str().as_bytes(), code.as_bytes()) {
            self.0.wrong_codes.fetch_add(1, Ordering::Relaxed);
            return if state.count_wrong(now) { PairStatus::TooManyAttempts } else { PairStatus::WrongCode };
        }
        state.window = None;
        state.wrong_in_a_row = 0;
        let device = state.store.add(name, key);
        tracing::info!(device = %device.name, id = %device.id, "device paired");
        PairStatus::Accepted
    }

    /// The paired device with `key`.
    pub fn find_device(&self, key: &[u8]) -> Option<PairedDevice> {
        lock(&self.0.state).store.find(key).cloned()
    }

    /// The paired devices.
    pub fn devices(&self) -> Vec<PairedDevice> {
        lock(&self.0.state).store.devices().to_vec()
    }

    /// Forgets device `id`; `false` if there was none.
    pub fn remove_device(&self, id: Uuid) -> bool {
        lock(&self.0.state).store.remove(id)
    }

    /// Wrong codes since start.
    pub fn wrong_codes(&self) -> u64 {
        self.0.wrong_codes.load(Ordering::Relaxed)
    }

    /// The responder of one connection, proving the device to `auth`. `host_fingerprint` is the
    /// SHA-256 of the host's certificate (what the device signs); `reply` sends the answer to
    /// an approval request once the Mac's user decided (HOST-15).
    pub fn responder(
        &self,
        host_fingerprint: [u8; 32],
        auth: ConnectionAuth,
        reply: ReplySender,
    ) -> Box<dyn Responder> {
        let mut nonce = [0_u8; NONCE_LEN];
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).is_err() {
            // Without randomness no challenge is safe: refuse rather than reuse a nonce.
            auth.refuse();
        }
        Box::new(HostResponder { pairing: self.clone(), nonce, host_fingerprint, auth, reply })
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---- one connection ------------------------------------------------------------------------------

/// Where one connection's authentication stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPhase {
    /// Not yet authenticated: nothing is shown, input is ignored.
    Pending,
    /// The device asked to be approved (request id); the Mac's user decides (HOST-15).
    AwaitingApproval(u64),
    /// The device paired or proved its key.
    Authorized(PairedDevice),
    /// Refused; the host drops the connection.
    Refused,
}

#[derive(Debug)]
struct AuthInner {
    phase: Mutex<AuthPhase>,
    changed: tokio::sync::Notify,
}

/// One connection's authentication state, shared by its responder, its display and its input
/// (clones share it).
#[derive(Debug, Clone)]
pub struct ConnectionAuth(Arc<AuthInner>);

impl ConnectionAuth {
    /// A pending connection.
    pub fn pending() -> Self {
        Self(Arc::new(AuthInner {
            phase: Mutex::new(AuthPhase::Pending),
            changed: tokio::sync::Notify::new(),
        }))
    }

    /// A connection that needs no authentication (pairing off).
    pub fn open() -> Self {
        let auth = Self::pending();
        *lock(&auth.0.phase) = AuthPhase::Authorized(PairedDevice {
            id: Uuid::nil(),
            name: "(pairing off)".into(),
            key: String::new(),
            paired_at: 0,
        });
        auth
    }

    /// The current phase.
    pub fn phase(&self) -> AuthPhase {
        lock(&self.0.phase).clone()
    }

    /// Whether the connection may see the desktop and send input.
    pub fn is_authorized(&self) -> bool {
        matches!(*lock(&self.0.phase), AuthPhase::Authorized(_))
    }

    fn set(&self, phase: AuthPhase) {
        *lock(&self.0.phase) = phase;
        self.0.changed.notify_waiters();
        self.0.changed.notify_one();
    }

    /// Authorizes `device`.
    pub fn authorize(&self, device: PairedDevice) {
        self.set(AuthPhase::Authorized(device));
    }

    /// Refuses the connection.
    pub fn refuse(&self) {
        self.set(AuthPhase::Refused);
    }

    /// The device waits for the Mac's user (request `id`).
    fn await_approval(&self, id: u64) {
        self.set(AuthPhase::AwaitingApproval(id));
    }

    /// Waits for the next change of phase.
    pub async fn changed(&self) {
        self.0.changed.notified().await;
    }
}

struct HostResponder {
    pairing: Pairing,
    nonce: [u8; NONCE_LEN],
    host_fingerprint: [u8; 32],
    auth: ConnectionAuth,
    reply: ReplySender,
}

impl std::fmt::Debug for HostResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostResponder").field("auth", &self.auth).finish_non_exhaustive()
    }
}

impl HostResponder {
    fn verified(&self, purpose: Purpose, key: &[u8], signature: &[u8]) -> bool {
        drift_apple::pairing::verify(
            key,
            &signed_bytes(purpose, &self.nonce, &self.host_fingerprint),
            signature,
        )
    }

    fn pair(&self, version: u32, code: &str, name: &str, key: &[u8], signature: &[u8]) -> PairStatus {
        if version != PAIRING_VERSION || !self.verified(Purpose::Pair, key, signature) {
            // A request that does not prove its key never reaches the code check.
            return PairStatus::WrongCode;
        }
        let name = if name.trim().is_empty() { "Unnamed device" } else { name.trim() };
        let status = self.pairing.try_pair(code, name, key);
        match (status, self.pairing.find_device(key)) {
            (PairStatus::Accepted, Some(device)) => self.auth.authorize(device),
            _ => self.auth.refuse(),
        }
        status
    }

    /// Asks the Mac's user; `None` while they decide (the reply is sent later), or the
    /// status to answer now.
    fn ask(&self, version: u32, name: &str, key: &[u8], signature: &[u8]) -> Option<PairStatus> {
        if version != PAIRING_VERSION || !self.verified(Purpose::Approve, key, signature) {
            // A request that does not prove its key never reaches the Mac's user.
            self.auth.refuse();
            return Some(PairStatus::Declined);
        }
        let name = if name.trim().is_empty() { "Unnamed device" } else { name.trim() };
        match self.pairing.ask(name, key, &self.auth, Arc::clone(&self.reply)) {
            Ok(id) => {
                self.auth.await_approval(id);
                None
            }
            Err(status) => {
                self.auth.refuse();
                Some(status)
            }
        }
    }

    fn authenticate(&self, key: &[u8], signature: &[u8]) -> AuthStatus {
        let status = match self.pairing.find_device(key) {
            None => AuthStatus::UnknownDevice,
            Some(_) if !self.verified(Purpose::Authenticate, key, signature) => AuthStatus::BadSignature,
            Some(device) => {
                tracing::info!(device = %device.name, "device authenticated");
                self.auth.authorize(device);
                return AuthStatus::Ok;
            }
        };
        tracing::info!(?status, "device refused");
        self.auth.refuse();
        status
    }
}

impl Responder for HostResponder {
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        if negotiated.capabilities.contains(Capabilities::PAIRING) {
            vec![PairingMessage::Challenge { nonce: self.nonce }.to_message()]
        } else {
            // A Drift client that cannot pair cannot authenticate either.
            tracing::info!("client does not offer pairing; refusing it");
            self.auth.refuse();
            Vec::new()
        }
    }

    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        let parsed = PairingMessage::from_message(message)?;
        let reply = match parsed {
            Ok(PairingMessage::PairRequest { version, code, device_name, device_key, signature })
                if self.auth.phase() == AuthPhase::Pending =>
            {
                PairingMessage::PairReply(self.pair(version, &code, &device_name, &device_key, &signature))
            }
            Ok(PairingMessage::AuthRequest { device_key, signature })
                if self.auth.phase() == AuthPhase::Pending =>
            {
                PairingMessage::AuthReply(self.authenticate(&device_key, &signature))
            }
            Ok(PairingMessage::ApprovalRequest { version, device_name, device_key, signature })
                if self.auth.phase() == AuthPhase::Pending =>
            {
                match self.ask(version, &device_name, &device_key, &signature) {
                    Some(status) => PairingMessage::PairReply(status),
                    None => return Some(Vec::new()),
                }
            }
            Ok(_) => return Some(Vec::new()),
            Err(e) => {
                tracing::warn!(error = %e, "malformed pairing message; refusing the connection");
                self.auth.refuse();
                return Some(Vec::new());
            }
        };
        Some(vec![reply.to_message()])
    }
}
