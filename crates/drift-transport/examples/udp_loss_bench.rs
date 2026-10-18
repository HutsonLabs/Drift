//! HOST-12 measurement: GFX-like frames over a lossy loopback link, three ways.
//!
//! * **RDP-UDP2** (`ironrdp-rdpeudp-tokio`: MS-RDPEUDP2 + TLS + RDPEMT tunnel), one message per
//!   frame. It is the only RDP-UDP mode the vendored tree implements: reliable and ordered.
//! * **QUIC, one stream per frame** (`drift_transport::quic`): a loss delays only its frame.
//! * **QUIC, one ordered stream** for all frames: same congestion control as the line above, but
//!   head-of-line blocking like TCP, so the difference between the two QUIC lines is what
//!   per-frame streams buy.
//!
//! Every frame carries its send time; the receiver records how late it completed. The link is
//! `drift_transport::lossy::LossyUdpProxy` (deterministic drops, fixed one-way delay).
//!
//! ```text
//! cargo run -p drift-transport --example udp_loss_bench -- [frames] [frame bytes]
//! ```
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use drift_testkit::TestCert;
use drift_transport::lossy::{LinkConfig, LossyUdpProxy};
use drift_transport::quic::{Congestion, QuicHost, connect};
use ironrdp_rdpemt::TunnelConfig;
use ironrdp_rdpeudp::ConnectionConfig;
use ironrdp_rdpeudp_tokio::{UdpAcceptConfig, UdpTransportConfig, accept_udp, connect_udp};
use tokio::net::UdpSocket;

const FPS: u64 = 60;
const ONE_WAY: Duration = Duration::from_millis(10);
const LOSSES: [f64; 5] = [0.0, 0.5, 1.0, 2.0, 5.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    RdpUdp2,
    QuicOrderedCubic,
    QuicPerFrameCubic,
    QuicOrderedBbr,
    QuicPerFrameBbr,
}

fn frame(id: u32, sent_us: u64, bytes: usize) -> Vec<u8> {
    let mut f = vec![u8::try_from(id % 251).unwrap_or(0); bytes.max(12)];
    f[..4].copy_from_slice(&id.to_be_bytes());
    f[4..12].copy_from_slice(&sent_us.to_be_bytes());
    f
}

fn stamp(f: &[u8]) -> (u32, u64) {
    (u32::from_be_bytes(f[..4].try_into().unwrap()), u64::from_be_bytes(f[4..12].try_into().unwrap()))
}

struct Stats {
    latencies_ms: Vec<f64>,
    missing: usize,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

async fn run(mode: Mode, loss: f64, frames: u32, bytes: usize) -> Stats {
    let cert = TestCert::generate("drift-host.local");
    let link = LinkConfig { loss_percent: loss, one_way_delay: ONE_WAY, jitter: Duration::ZERO, seed: 11 };
    let base = Instant::now();
    let now_us = move || u64::try_from(base.elapsed().as_micros()).unwrap_or(u64::MAX);
    let interval = Duration::from_micros(1_000_000 / FPS);
    let deadline = Duration::from_secs(u64::from(frames) / FPS + 10);
    let mut latencies = Vec::new();

    match mode {
        Mode::RdpUdp2 => {
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let host_addr = socket.local_addr().unwrap();
            let proxy = LossyUdpProxy::start(host_addr, link).await.unwrap();
            let tunnel = TunnelConfig { request_id: 7, security_cookie: [0x5A; 16] };
            let tls = Arc::new(
                tokio_rustls::rustls::ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(
                        vec![cert.der().to_vec().into()],
                        tokio_rustls::rustls::pki_types::PrivateKeyDer::try_from(cert.key_pkcs8().to_vec())
                            .unwrap(),
                    )
                    .unwrap(),
            );
            let server = tokio::spawn({
                let tunnel = tunnel.clone();
                async move {
                    let host = accept_udp(
                        socket,
                        UdpAcceptConfig {
                            tls_config: tls,
                            tunnel_config: tunnel,
                            connection_config: ConnectionConfig::default(),
                            accept_timeout: Duration::from_secs(20),
                        },
                    )
                    .await
                    .unwrap();
                    let mut tick = tokio::time::interval(interval);
                    for id in 0..frames {
                        tick.tick().await;
                        // Large frames are split: the tunnel carries messages below 64 KiB.
                        let _ = host.send(frame(id, now_us(), bytes)).await;
                    }
                    tokio::time::sleep(deadline).await;
                }
            });
            let mut client =
                connect_udp(UdpTransportConfig::new(proxy.addr(), "drift-host.local".into(), tunnel))
                    .await
                    .unwrap();
            let end = tokio::time::Instant::now() + deadline;
            while latencies.len() < frames as usize {
                let Ok(Some(f)) = tokio::time::timeout_at(end, client.recv()).await else { break };
                let (_, sent) = stamp(&f);
                #[expect(clippy::cast_precision_loss)]
                latencies.push((now_us().saturating_sub(sent)) as f64 / 1000.0);
            }
            server.abort();
        }
        Mode::QuicPerFrameCubic | Mode::QuicOrderedCubic | Mode::QuicPerFrameBbr | Mode::QuicOrderedBbr => {
            let congestion = if matches!(mode, Mode::QuicPerFrameBbr | Mode::QuicOrderedBbr) {
                Congestion::Bbr
            } else {
                Congestion::Cubic
            };
            let per_frame = matches!(mode, Mode::QuicPerFrameBbr | Mode::QuicPerFrameCubic);
            let host = QuicHost::bind_with(
                "127.0.0.1:0".parse().unwrap(),
                cert.der().to_vec(),
                cert.key_pkcs8().to_vec(),
                congestion,
            )
            .unwrap();
            let proxy_bypass = host.local_addr();
            let proxy = LossyUdpProxy::start(host.local_addr(), link).await.unwrap();
            let server = tokio::spawn(async move {
                let mut session = host.accept().await.unwrap();
                let mut hello = [0u8; 1];
                session.main_recv().read_exact(&mut hello).await.unwrap();
                let mut tick = tokio::time::interval(interval);
                for id in 0..frames {
                    tick.tick().await;
                    let f = frame(id, now_us(), bytes);
                    if per_frame {
                        let _ = session.send_frame(id, &f).await;
                    } else {
                        let len = u32::try_from(f.len()).unwrap().to_be_bytes();
                        let _ = session.main_send().write_all(&len).await;
                        let _ = session.main_send().write_all(&f).await;
                    }
                }
                tokio::time::sleep(deadline).await;
                drop(host);
            });
            let target = if std::env::var_os("BENCH_DIRECT").is_some() { proxy_bypass } else { proxy.addr() };
            let mut client = connect(target, "drift-host.local", cert.fingerprint()).await.unwrap();
            client.main_send().write_all(&[1]).await.unwrap();
            let end = tokio::time::Instant::now() + deadline;
            while latencies.len() < frames as usize {
                let f = if per_frame {
                    let Ok(Some((_, f))) = tokio::time::timeout_at(end, client.recv_frame()).await else {
                        break;
                    };
                    f
                } else {
                    let mut len = [0u8; 4];
                    let Ok(Ok(())) =
                        tokio::time::timeout_at(end, client.main_recv().read_exact(&mut len)).await
                    else {
                        break;
                    };
                    let mut f = vec![0u8; u32::from_be_bytes(len) as usize];
                    let Ok(Ok(())) =
                        tokio::time::timeout_at(end, client.main_recv().read_exact(&mut f)).await
                    else {
                        break;
                    };
                    f
                };
                let (_, sent) = stamp(&f);
                #[expect(clippy::cast_precision_loss)]
                latencies.push((now_us().saturating_sub(sent)) as f64 / 1000.0);
            }
            server.abort();
        }
    }
    let missing = frames as usize - latencies.len();
    latencies.sort_by(f64::total_cmp);
    Stats { latencies_ms: latencies, missing }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let mut args = std::env::args().skip(1);
    let frames: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(600);
    let bytes: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(20_000);
    println!(
        "{frames} frames of {bytes} bytes at {FPS} fps, one-way delay {} ms; latency from send to complete frame",
        ONE_WAY.as_millis()
    );
    println!("| transport | loss | p50 ms | p95 ms | p99 ms | max ms | frames > 100 ms | lost |");
    println!("|---|---|---|---|---|---|---|---|");
    for loss in LOSSES {
        for mode in [
            Mode::RdpUdp2,
            Mode::QuicOrderedCubic,
            Mode::QuicPerFrameCubic,
            Mode::QuicOrderedBbr,
            Mode::QuicPerFrameBbr,
        ] {
            let s = run(mode, loss, frames, bytes).await;
            let late = s.latencies_ms.iter().filter(|l| **l > 100.0).count();
            println!(
                "| {mode:?} | {loss}% | {:.1} | {:.1} | {:.1} | {:.1} | {late} | {} |",
                percentile(&s.latencies_ms, 0.5),
                percentile(&s.latencies_ms, 0.95),
                percentile(&s.latencies_ms, 0.99),
                s.latencies_ms.last().copied().unwrap_or(f64::NAN),
                s.missing,
            );
        }
    }
}
