//! Which transports a client tries, in which order (ADR `IOS-0` decision 6).
//!
//! A paired Drift Host is tried over QUIC first, then over TCP; every other host (GNOME, any
//! RDP server) over TCP only. Each attempt gets a timeout, because a firewall that drops UDP
//! makes QUIC wait rather than fail.

use std::future::Future;
use std::time::Duration;

/// A transport the client can connect over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// RDP over QUIC (Drift-to-Drift only).
    Quic,
    /// RDP over TLS over TCP (every host).
    Tcp,
}

/// The order to try transports in.
pub fn transports_for(paired_drift_host: bool) -> Vec<Transport> {
    if paired_drift_host { vec![Transport::Quic, Transport::Tcp] } else { vec![Transport::Tcp] }
}

/// A successful connection and the attempts that failed before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt<T> {
    /// The transport that worked.
    pub transport: Transport,
    /// What its connector returned.
    pub value: T,
    /// Earlier transports and why each failed.
    pub failures: Vec<(Transport, String)>,
}

/// Tries `order` in turn, each within `timeout`; returns the first success, or every failure.
pub async fn connect_with_fallback<T, F, Fut>(
    order: &[Transport],
    timeout: Duration,
    mut connect: F,
) -> Result<Attempt<T>, Vec<(Transport, String)>>
where
    F: FnMut(Transport) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let mut failures = Vec::new();
    for &transport in order {
        match tokio::time::timeout(timeout, connect(transport)).await {
            Ok(Ok(value)) => return Ok(Attempt { transport, value, failures }),
            Ok(Err(e)) => failures.push((transport, e)),
            Err(_) => failures.push((transport, format!("no answer within {timeout:?}"))),
        }
        if let Some((t, why)) = failures.last() {
            tracing::info!(transport = ?t, %why, "transport attempt failed");
        }
    }
    Err(failures)
}
