//! An in-process SSH server for `drift-ssh`'s tests (term.hut's `test_server`, one step closer
//! to sshd): public keys are checked against `$HOME/.ssh/authorized_keys`, and `exec` runs the
//! command with `sh -c` under that HOME, with stdin, stdout, stderr and the exit status wired
//! through the channel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::ChildStdin;

/// What the server accepts and where its commands run.
#[derive(Clone)]
pub struct ServerOptions {
    /// The account's HOME (its `.ssh/authorized_keys` decides public keys).
    pub home: PathBuf,
    /// The account's password; `None`: no password logins.
    pub password: Option<String>,
    /// Put first on the commands' PATH (fake `sudo`).
    pub path_prefix: Option<PathBuf>,
    /// Seed of the ed25519 host key (another seed is another machine).
    pub host_key_seed: u8,
}

impl ServerOptions {
    pub fn new(home: &Path) -> Self {
        Self { home: home.to_owned(), password: None, path_prefix: None, host_key_seed: 1 }
    }
}

pub struct TestServer {
    pub port: u16,
    /// `SHA256:…` of the host key.
    pub fingerprint: String,
}

pub fn host_key(seed: u8) -> PrivateKey {
    let pair = Ed25519Keypair::from_seed(&[seed; 32]);
    PrivateKey::new(KeypairData::Ed25519(pair), "").expect("host key")
}

impl TestServer {
    pub async fn start(options: ServerOptions) -> TestServer {
        let key = host_key(options.host_key_seed);
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let mut methods = vec![MethodKind::PublicKey];
        if options.password.is_some() {
            methods.push(MethodKind::Password);
        }
        let config = Arc::new(server::Config {
            keys: vec![key],
            methods: MethodSet::from(methods.as_slice()),
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                let handler = Conn { options: options.clone(), stdin: Arc::default() };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, socket, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });
        TestServer { port, fingerprint }
    }
}

struct Conn {
    options: ServerOptions,
    stdin: Arc<Mutex<HashMap<ChannelId, ChildStdin>>>,
}

impl Conn {
    /// Like sshd: a failed attempt leaves every method on offer (russh's default drops the one
    /// that failed, so a wrong password would read as "no password logins").
    fn reject(&self) -> Auth {
        let mut methods = vec![MethodKind::PublicKey];
        if self.options.password.is_some() {
            methods.push(MethodKind::Password);
        }
        Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(methods.as_slice())),
            partial_success: false,
        }
    }
}

fn authorized(home: &Path, key: &PublicKey) -> bool {
    let Ok(text) = std::fs::read_to_string(home.join(".ssh/authorized_keys")) else { return false };
    text.lines()
        .filter_map(|l| PublicKey::from_openssh(l.trim()).ok())
        .any(|k| k.key_data() == key.key_data())
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if self.options.password.as_deref() == Some(password) { Auth::Accept } else { self.reject() })
    }

    async fn auth_publickey(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if authorized(&self.options.home, key) { Auth::Accept } else { self.reject() })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        let mut path = std::env::var("PATH").unwrap_or_default();
        if let Some(prefix) = &self.options.path_prefix {
            path = format!("{}:{path}", prefix.display());
        }
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env("HOME", &self.options.home)
            .env("PATH", path)
            .current_dir(&self.options.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh");
        if let Some(stdin) = child.stdin.take() {
            self.stdin.lock().expect("stdin map").insert(channel, stdin);
        }
        let (mut out, mut err) = (child.stdout.take().expect("stdout"), child.stderr.take().expect("stderr"));
        let handle = session.handle();
        session.channel_success(channel)?;
        tokio::spawn(async move {
            let h = handle.clone();
            let stdout = tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                while let Ok(n @ 1..) = out.read(&mut buf).await {
                    let _ = h.data(channel, buf[..n].to_vec()).await;
                }
            });
            let h = handle.clone();
            let stderr = tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                while let Ok(n @ 1..) = err.read(&mut buf).await {
                    let _ = h.extended_data(channel, 1, buf[..n].to_vec()).await;
                }
            });
            let _ = stdout.await;
            let _ = stderr.await;
            let status = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(255);
            let _ = handle.exit_status_request(channel, u32::try_from(status).unwrap_or(255)).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let stdin = self.stdin.lock().expect("stdin map").remove(&channel);
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(data).await;
            self.stdin.lock().expect("stdin map").insert(channel, stdin);
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        // Dropping the pipe is the child's EOF.
        drop(self.stdin.lock().expect("stdin map").remove(&channel));
        Ok(())
    }
}
