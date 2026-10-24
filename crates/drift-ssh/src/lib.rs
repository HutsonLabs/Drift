//! In-app SSH for setting up a Linux machine (ADR `GNOME-WIZARD-headless-setup`), the way
//! term.hut connects on iPhone: russh in-process, one ed25519 device key in the app's own
//! directory, a known_hosts file next to it, strict host keys with an explicit "trust", and the
//! account password used once to install the device key. Nothing calls a system tool or reads
//! `~/.ssh`, so it works inside the App Sandbox and on iOS.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use russh::client::{self, AuthResult, Handle};
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, MethodKind};
use serde::{Deserialize, Serialize};

/// How long connecting and signing in may take.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A local failure of the key store.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// Reading or writing the store's files.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The device key could not be made or read.
    #[error("device key: {0}")]
    Key(String),
}

/// The app's SSH files: the device key (`id_ed25519`, mode 0600) and `known_hosts`.
#[derive(Debug, Clone)]
pub struct KeyStore {
    dir: PathBuf,
}

impl KeyStore {
    /// Uses `dir` (created with mode 0700) for the store.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, SshError> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir })
    }

    /// The device key's OpenSSH public line; the key is made on first use, labelled `comment`.
    pub fn public_key(&self, comment: &str) -> Result<String, SshError> {
        self.device_key(comment)?.public_key().to_openssh().map_err(|e| SshError::Key(e.to_string()))
    }

    /// The device key, made on first use: 32 random bytes as an ed25519 seed, written 0600 to a
    /// temp file and renamed into place, so a crash never leaves half a key or a readable one.
    fn device_key(&self, comment: &str) -> Result<PrivateKey, SshError> {
        // Two first calls at once must not each make a key.
        static MAKING: Mutex<()> = Mutex::new(());
        let _one = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.dir.join("id_ed25519");
        if !path.exists() {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| SshError::Key(format!("no randomness: {e}")))?;
            let pair = Ed25519Keypair::from_seed(&seed);
            seed.fill(0);
            let key = PrivateKey::new(KeypairData::Ed25519(pair), comment)
                .map_err(|e| SshError::Key(e.to_string()))?;
            let text = key.to_openssh(LineEnding::LF).map_err(|e| SshError::Key(e.to_string()))?;
            write_private(&path, text.as_bytes())?;
        }
        let text = std::fs::read_to_string(&path)?;
        PrivateKey::from_openssh(&text).map_err(|e| SshError::Key(e.to_string()))
    }

    /// Forgets the recorded host key of `host:port`.
    pub fn forget_host(&self, host: &str, port: u16) -> Result<(), SshError> {
        let path = self.known_hosts();
        let Ok(text) = std::fs::read_to_string(&path) else { return Ok(()) };
        let name = if port == 22 { host.to_owned() } else { format!("[{host}]:{port}") };
        let mut kept = String::new();
        for line in text.lines() {
            let names = line.split_whitespace().next().unwrap_or("");
            if !names.split(',').any(|n| n == name) {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        write_private(&path, kept.as_bytes())?;
        Ok(())
    }

    /// The store's known_hosts file.
    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }
}

/// Writes `bytes` to `path` with mode 0600 from the first byte, via a temp file and a rename.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let tmp = path.with_extension("tmp");
    let mut file =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The machine and account to sign in to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Target {
    /// Host name or address.
    pub host: String,
    /// SSH port.
    pub port: u16,
    /// Account.
    pub user: String,
}

/// What to do with a host key that is not recorded yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// Stop and report it.
    KnownOnly,
    /// Record it (the wizard's "Trust and Connect").
    TrustNew,
}

/// Why connecting, signing in or running a command failed, with the one fix the wizard offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Diagnosis {
    /// No TCP connection (refused, no route, name not found).
    #[error("cannot reach the machine: {detail}")]
    Unreachable {
        /// The system's reason.
        detail: String,
    },
    /// Connecting or signing in took longer than [`CONNECT_TIMEOUT`].
    #[error("the machine did not answer in time")]
    TimedOut,
    /// The host key is not recorded yet.
    #[error("unknown host key {fingerprint}")]
    HostKeyUnknown {
        /// `SHA256:…`.
        fingerprint: String,
    },
    /// The host key differs from the recorded one.
    #[error("the host key changed to {fingerprint}")]
    HostKeyChanged {
        /// `SHA256:…` of the new key.
        fingerprint: String,
    },
    /// The account does not accept the device key.
    #[error("the account does not accept Drift's key")]
    KeyNotAuthorized {
        /// Whether a password login is offered (to install the key with).
        offers_password: bool,
    },
    /// The password was wrong.
    #[error("the password was not accepted")]
    PasswordRejected,
    /// The server does not take passwords.
    #[error("the machine does not accept password logins")]
    PasswordNotOffered,
    /// The connection ended before the command did.
    #[error("the connection was lost")]
    ConnectionLost,
    /// Anything else.
    #[error("{detail}")]
    Failed {
        /// What happened.
        detail: String,
    },
}

impl Diagnosis {
    fn failed(e: impl std::fmt::Display) -> Self {
        Self::Failed { detail: e.to_string() }
    }
}

/// Host-key checking against the store's known_hosts. russh's handler can only answer yes or
/// no, so the reason for a no is left in `issue` for [`dial`] to report.
struct Client {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    trust: Trust,
    issue: Arc<Mutex<Option<Diagnosis>>>,
}

impl Client {
    fn refuse(&self, why: Diagnosis) -> Result<bool, russh::Error> {
        *self.issue.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why);
        Ok(false)
    }
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return self.refuse(Diagnosis::failed(
                "the machine offers a host certificate; Drift checks plain host keys",
            ));
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        match check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) if self.trust == Trust::TrustNew => {
                match learn_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
                    Ok(()) => Ok(true),
                    Err(e) => self.refuse(Diagnosis::failed(format!("cannot record the host key: {e}"))),
                }
            }
            Ok(false) => self.refuse(Diagnosis::HostKeyUnknown { fingerprint }),
            // A changed key, or a line that does not parse: never accepted, only forgotten.
            Err(_) => self.refuse(Diagnosis::HostKeyChanged { fingerprint }),
        }
    }
}

/// TCP and the SSH handshake with host-key checking, within [`CONNECT_TIMEOUT`].
async fn dial(keys: &KeyStore, target: &Target, trust: Trust) -> Result<Handle<Client>, Diagnosis> {
    let issue = Arc::new(Mutex::new(None));
    let client = Client {
        host: target.host.clone(),
        port: target.port,
        known_hosts: keys.known_hosts(),
        trust,
        issue: issue.clone(),
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(10)),
        keepalive_max: 3,
        ..client::Config::default()
    });
    let socket = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((target.host.as_str(), target.port)),
    )
    .await
    .map_err(|_| Diagnosis::TimedOut)?
    .map_err(|e| Diagnosis::Unreachable { detail: e.to_string() })?;
    match tokio::time::timeout(CONNECT_TIMEOUT, client::connect_stream(config, socket, client)).await {
        Err(_) => Err(Diagnosis::TimedOut),
        Ok(Ok(handle)) => Ok(handle),
        Ok(Err(e)) => {
            let why = issue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            Err(why.unwrap_or_else(|| Diagnosis::failed(e)))
        }
    }
}

/// A signed-in SSH connection.
pub struct Connection {
    handle: Handle<Client>,
}

/// Connects to `target` and signs in with the device key.
pub async fn connect(
    keys: &KeyStore,
    target: &Target,
    trust: Trust,
    comment: &str,
) -> Result<Connection, Diagnosis> {
    let key = keys.device_key(comment).map_err(Diagnosis::failed)?;
    let mut handle = dial(keys, target, trust).await?;
    let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
    let auth =
        tokio::time::timeout(CONNECT_TIMEOUT, handle.authenticate_publickey(target.user.as_str(), key))
            .await
            .map_err(|_| Diagnosis::TimedOut)?
            .map_err(Diagnosis::failed)?;
    match auth {
        AuthResult::Success => Ok(Connection { handle }),
        AuthResult::Failure { remaining_methods, .. } => Err(Diagnosis::KeyNotAuthorized {
            offers_password: remaining_methods.contains(&MethodKind::Password),
        }),
    }
}

/// Signs in once with `password` and appends the device key to the account's
/// `~/.ssh/authorized_keys` (idempotent). The password is not kept.
pub async fn install_device_key(
    keys: &KeyStore,
    target: &Target,
    password: &str,
    comment: &str,
) -> Result<(), Diagnosis> {
    let public = keys.public_key(comment).map_err(Diagnosis::failed)?;
    // Installing never trusts a host key on its own: that is the wizard's explicit step.
    let mut handle = dial(keys, target, Trust::KnownOnly).await?;
    let auth =
        tokio::time::timeout(CONNECT_TIMEOUT, handle.authenticate_password(target.user.as_str(), password))
            .await
            .map_err(|_| Diagnosis::TimedOut)?
            .map_err(Diagnosis::failed)?;
    match auth {
        AuthResult::Success => {}
        AuthResult::Failure { remaining_methods, .. } => {
            return Err(if remaining_methods.contains(&MethodKind::Password) {
                Diagnosis::PasswordRejected
            } else {
                Diagnosis::PasswordNotOffered
            });
        }
    }
    let conn = Connection { handle };
    // The key arrives on stdin, so nothing but a fixed command is on the command line.
    let command = "umask 077 && mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && \
                   IFS= read -r key && { grep -qxF \"$key\" ~/.ssh/authorized_keys || \
                   printf '%s\\n' \"$key\" >> ~/.ssh/authorized_keys; }";
    let mut output = Vec::new();
    let status =
        conn.run(command, format!("{public}\n").as_bytes(), &mut |l| output.push(l.to_owned())).await;
    conn.close().await;
    match status? {
        Some(0) => Ok(()),
        _ => Err(Diagnosis::failed(format!("adding the key failed: {}", output.join("; ")))),
    }
}

/// Collects one stream's bytes and hands out complete lines.
#[derive(Default)]
struct Lines(Vec<u8>);

impl Lines {
    fn push(&mut self, bytes: &[u8], on_line: &mut (dyn FnMut(&str) + Send)) {
        self.0.extend_from_slice(bytes);
        while let Some(end) = self.0.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.0.drain(..=end).collect();
            emit(&line[..end], on_line);
        }
    }

    fn finish(&mut self, on_line: &mut (dyn FnMut(&str) + Send)) {
        if !self.0.is_empty() {
            emit(&std::mem::take(&mut self.0), on_line);
        }
    }
}

fn emit(line: &[u8], on_line: &mut (dyn FnMut(&str) + Send)) {
    let line = String::from_utf8_lossy(line);
    on_line(line.strip_suffix('\r').unwrap_or(&line));
}

impl Connection {
    /// Runs `command`, writes `stdin` and closes it, and calls `on_line` for every line of
    /// output (stdout and stderr, without the newline). Returns the exit status (`None` when the
    /// server sent none).
    pub async fn run(
        &self,
        command: &str,
        stdin: &[u8],
        on_line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Option<u32>, Diagnosis> {
        let channel = self.handle.channel_open_session().await.map_err(Diagnosis::failed)?;
        channel.exec(true, command).await.map_err(Diagnosis::failed)?;
        let (mut read, write) = channel.split();
        // Write and read at once: a command that answers before it has read all of stdin must
        // not stall on a full window.
        let send = async {
            write.data(stdin).await?;
            write.eof().await
        };
        let receive = async {
            let (mut out, mut err) = (Lines::default(), Lines::default());
            let (mut status, mut eof) = (None, false);
            let ended = loop {
                match read.wait().await {
                    Some(ChannelMsg::Data { data }) => out.push(&data, on_line),
                    Some(ChannelMsg::ExtendedData { data, .. }) => err.push(&data, on_line),
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        status = Some(exit_status);
                        if eof {
                            break true;
                        }
                    }
                    // The status follows EOF; Close ends the command either way.
                    Some(ChannelMsg::Eof) => {
                        eof = true;
                        if status.is_some() {
                            break true;
                        }
                    }
                    Some(ChannelMsg::Close) => break true,
                    Some(_) => {}
                    // The channel just ending is the connection dying mid-command, not an answer.
                    None => break false,
                }
            };
            out.finish(on_line);
            err.finish(on_line);
            (ended, status)
        };
        let (sent, (ended, status)) = tokio::join!(send, receive);
        if !ended {
            return Err(Diagnosis::ConnectionLost);
        }
        // A command may exit without reading all of stdin; only a lost link is an error then.
        if sent.is_err() && status.is_none() {
            return Err(Diagnosis::ConnectionLost);
        }
        Ok(status)
    }

    /// Whether the account can use sudo without a password (`sudo -n true`).
    pub async fn sudo_without_password(&self) -> Result<bool, Diagnosis> {
        Ok(self.run("sudo -n true", &[], &mut |_| {}).await? == Some(0))
    }

    /// Ends the connection.
    pub async fn close(self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
    }
}
