//! A lossy link for measurements and tests: a deterministic drop model and a loopback UDP proxy
//! that applies it with a fixed delay (task HOST-12).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

/// Drops a configured share of packets, deterministically for a seed (SplitMix64).
#[derive(Debug, Clone)]
pub struct LossModel {
    threshold: f64,
    state: u64,
}

impl LossModel {
    /// Drops `loss_percent` % of packets (clamped to 0..=100).
    pub fn new(loss_percent: f64, seed: u64) -> Self {
        Self { threshold: (loss_percent / 100.0).clamp(0.0, 1.0), state: seed }
    }

    fn next_unit(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // 53 random bits → [0, 1).
        #[expect(clippy::cast_precision_loss, reason = "53-bit value, exactly representable")]
        let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
        unit
    }

    /// Whether the next packet is dropped.
    pub fn drop_next(&mut self) -> bool {
        self.threshold > 0.0 && self.next_unit() < self.threshold
    }
}

/// How the proxy treats packets in each direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkConfig {
    /// Share of packets dropped, in percent.
    pub loss_percent: f64,
    /// Delay added to every forwarded packet.
    pub one_way_delay: Duration,
    /// Extra random delay, up to this much. Packets still leave in arrival order, so jitter
    /// delays the packets behind a late one too.
    pub jitter: Duration,
    /// Seed of the drop model (the reverse direction uses `seed + 1`).
    pub seed: u64,
}

/// Packet counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProxyStats {
    /// Packets forwarded.
    pub forwarded: u64,
    /// Packets dropped.
    pub dropped: u64,
    /// Socket errors seen (and skipped).
    pub errors: u64,
}

#[derive(Default)]
struct Counters {
    forwarded: AtomicU64,
    dropped: AtomicU64,
    errors: AtomicU64,
}

/// A UDP proxy on `127.0.0.1` between one client and `upstream`, applying a [`LinkConfig`].
/// Stops when dropped.
pub struct LossyUdpProxy {
    addr: SocketAddr,
    counters: Arc<Counters>,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for LossyUdpProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LossyUdpProxy").field("addr", &self.addr).field("stats", &self.stats()).finish()
    }
}

impl Drop for LossyUdpProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LossyUdpProxy {
    /// Starts forwarding to `upstream`; clients send to [`Self::addr`].
    pub async fn start(upstream: SocketAddr, link: LinkConfig) -> io::Result<Self> {
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        let back = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        back.connect(upstream).await?;
        let addr = front.local_addr()?;
        let counters = Arc::new(Counters::default());
        let client: Arc<Mutex<Option<SocketAddr>>> = Arc::default();

        // One delay line per direction: packets leave in the order they arrived. (Spawning a
        // sleeping task per packet reorders them, which QUIC and RDP-UDP read as loss.)
        let (up_tx, up_rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
        let (down_tx, down_rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();

        let task = tokio::spawn({
            let (front, back, counters, client) =
                (front.clone(), back.clone(), counters.clone(), client.clone());
            async move {
                let up_line = delay_line(up_rx, {
                    let back = back.clone();
                    move |packet: Vec<u8>| {
                        let back = back.clone();
                        async move { send_retrying(|| back.send(&packet)).await }
                    }
                });
                let down_line = delay_line(down_rx, {
                    let (front, client) = (front.clone(), client.clone());
                    move |packet: Vec<u8>| {
                        let (front, client) = (front.clone(), client.clone());
                        async move {
                            let Some(to) = *client.lock().await else { return };
                            send_retrying(|| front.send_to(&packet, to)).await;
                        }
                    }
                });
                let from_upstream = {
                    let (back, counters) = (back.clone(), counters.clone());
                    async move {
                        let mut model = LossModel::new(link.loss_percent, link.seed.wrapping_add(1));
                        let mut buf = vec![0u8; 65_536];
                        loop {
                            // A UDP socket reports earlier send failures (ICMP, ENOBUFS) on a
                            // later receive; a link keeps going after them.
                            match back.recv(&mut buf).await {
                                Ok(n) => admit(&mut model, &counters, link, &down_tx, &buf[..n]),
                                Err(e) => {
                                    tracing::debug!(error = %e, "lossy proxy: upstream receive");
                                    counters.errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                };
                let from_client = async move {
                    let mut model = LossModel::new(link.loss_percent, link.seed);
                    let mut buf = vec![0u8; 65_536];
                    loop {
                        match front.recv_from(&mut buf).await {
                            Ok((n, from)) => {
                                *client.lock().await = Some(from);
                                admit(&mut model, &counters, link, &up_tx, &buf[..n]);
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, "lossy proxy: client receive");
                                counters.errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                };
                tokio::join!(up_line, down_line, from_upstream, from_client);
            }
        });
        Ok(Self { addr, counters, task })
    }

    /// The address clients send to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Counters so far.
    pub fn stats(&self) -> ProxyStats {
        ProxyStats {
            forwarded: self.counters.forwarded.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
        }
    }
}

/// Sends one datagram, retrying briefly while the kernel reports a full queue (`ENOBUFS` on
/// macOS loopback under bursts); a real link would queue the packet, not lose it.
async fn send_retrying<F, Fut>(mut send: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = io::Result<usize>>,
{
    for _ in 0..200 {
        match send().await {
            Ok(_) => return,
            Err(e) if e.raw_os_error() == Some(55) => tokio::time::sleep(Duration::from_micros(200)).await,
            Err(e) => {
                tracing::debug!(error = %e, "lossy proxy: send");
                return;
            }
        }
    }
}

/// Drops the packet or queues it on the delay line with its departure time.
fn admit(
    model: &mut LossModel,
    counters: &Counters,
    link: LinkConfig,
    line: &mpsc::UnboundedSender<(Instant, Vec<u8>)>,
    packet: &[u8],
) {
    if model.drop_next() {
        counters.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    counters.forwarded.fetch_add(1, Ordering::Relaxed);
    let delay = link.one_way_delay + link.jitter.mul_f64(model.next_unit());
    let _ = line.send((Instant::now() + delay, packet.to_vec()));
}

/// Sends queued packets in order, each no earlier than its departure time.
async fn delay_line<F, Fut>(mut rx: mpsc::UnboundedReceiver<(Instant, Vec<u8>)>, send: F)
where
    F: Fn(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    while let Some((due, packet)) = rx.recv().await {
        tokio::time::sleep_until(due).await;
        send(packet).await;
    }
}
