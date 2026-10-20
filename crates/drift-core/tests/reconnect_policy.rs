//! M7-1 Red: seeded-RNG delay tables, bounds proptest, exhaustive classification and
//! reset-after-stable for `ReconnectPolicy`.

use std::time::{Duration, Instant};

use drift_core::DisconnectReason;
use drift_core::reconnect::{GiveUpReason, ReconnectConfig, ReconnectDecision, ReconnectPolicy, SplitMix64};
use proptest::prelude::*;

fn unlimited() -> ReconnectConfig {
    ReconnectConfig { max_attempts: None, ..ReconnectConfig::default() }
}

fn delays(seed: u64, n: usize) -> Vec<u64> {
    let mut p = ReconnectPolicy::new(unlimited(), seed);
    let t = Instant::now();
    (1..=n)
        .map(|i| match p.on_disconnect(&DisconnectReason::Network, t) {
            ReconnectDecision::Retry { attempt, delay } => {
                assert_eq!(attempt as usize, i);
                delay.as_millis() as u64
            }
            other => panic!("attempt {i}: {other:?}"),
        })
        .collect()
}

#[test]
fn splitmix64_matches_reference_vectors() {
    // Published SplitMix64 outputs for seed 0.
    let mut r = SplitMix64::new(0);
    assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
    assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    assert_eq!(r.next_u64(), 0x06C4_5D18_8009_454F);
}

#[test]
fn seeded_delay_tables() {
    // Reference values computed independently (Python) from:
    // ceiling_n = min(30000, 500 * 2^(n-1)) ms; delay_n = (splitmix64_n * (ceiling_n + 1)) >> 64.
    #[rustfmt::skip]
    let tables: &[(u64, [u64; 12])] = &[
        (0,      [442, 431, 52, 3884, 850, 5237, 5216, 23147, 7370, 28561, 11894, 22831]),
        (42,     [371, 160, 557, 1377, 304, 13892, 6552, 24019, 10198, 18555, 6147, 14790]),
        (0xD1F7, [369, 525, 1099, 1751, 7767, 12003, 3655, 8751, 3696, 9480, 24084, 17697]),
    ];
    for (seed, expected) in tables {
        assert_eq!(delays(*seed, 12), expected.to_vec(), "seed {seed}");
    }
}

#[test]
fn ceilings_double_from_500ms_and_cap_at_30s() {
    let p = ReconnectPolicy::new(ReconnectConfig::default(), 0);
    let ms: Vec<u128> = (1..=9).map(|n| p.ceiling(n).as_millis()).collect();
    assert_eq!(ms, [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
    assert_eq!(p.ceiling(0), Duration::from_millis(500));
    assert_eq!(p.ceiling(u32::MAX), Duration::from_secs(30));
}

#[test]
fn default_budget_is_twenty_attempts() {
    let mut p = ReconnectPolicy::new(ReconnectConfig::default(), 7);
    let t = Instant::now();
    for i in 1..=20 {
        assert!(
            matches!(p.on_disconnect(&DisconnectReason::Timeout, t), ReconnectDecision::Retry { attempt, .. } if attempt == i)
        );
    }
    assert_eq!(
        p.on_disconnect(&DisconnectReason::Timeout, t),
        ReconnectDecision::GiveUp(GiveUpReason::AttemptsExhausted)
    );
    assert_eq!(p.attempts(), 20);
}

/// Exhaustive: adding a `DisconnectReason` variant breaks this match until it is classified.
fn expected_retryable(r: &DisconnectReason) -> bool {
    match r {
        DisconnectReason::Network
        | DisconnectReason::TlsEof
        | DisconnectReason::ServerShutdown
        | DisconnectReason::Timeout => true,
        DisconnectReason::AuthFailed
        | DisconnectReason::RdstlsFailed(_)
        | DisconnectReason::CertMismatch
        | DisconnectReason::ProtocolError(_)
        | DisconnectReason::RedirectLoop
        | DisconnectReason::UserClosed
        | DisconnectReason::LoggedOffRemotely
        | DisconnectReason::LocalNetworkDenied => false,
    }
}

#[test]
fn classification_is_exhaustive() {
    let all = [
        DisconnectReason::Network,
        DisconnectReason::TlsEof,
        DisconnectReason::ServerShutdown,
        DisconnectReason::Timeout,
        DisconnectReason::AuthFailed,
        DisconnectReason::RdstlsFailed(0x52E),
        DisconnectReason::CertMismatch,
        DisconnectReason::ProtocolError("x".into()),
        DisconnectReason::RedirectLoop,
        DisconnectReason::UserClosed,
        DisconnectReason::LoggedOffRemotely,
        DisconnectReason::LocalNetworkDenied,
    ];
    for reason in &all {
        let mut p = ReconnectPolicy::new(ReconnectConfig::default(), 1);
        let d = p.on_disconnect(reason, Instant::now());
        if expected_retryable(reason) {
            assert!(matches!(d, ReconnectDecision::Retry { attempt: 1, .. }), "{reason:?}: {d:?}");
        } else {
            assert_eq!(d, ReconnectDecision::GiveUp(GiveUpReason::NotRetryable), "{reason:?}");
            assert_eq!(p.attempts(), 0, "{reason:?} must not consume an attempt");
        }
    }
}

#[test]
fn attempts_reset_after_sixty_seconds_stable() {
    let mut p = ReconnectPolicy::new(ReconnectConfig::default(), 3);
    let t0 = Instant::now();
    for _ in 0..3 {
        p.on_disconnect(&DisconnectReason::Network, t0);
    }
    assert_eq!(p.attempts(), 3);

    // Connected for 59 s: not stable, the count continues.
    p.on_connected(t0);
    let d = p.on_disconnect(&DisconnectReason::Network, t0 + Duration::from_secs(59));
    assert!(matches!(d, ReconnectDecision::Retry { attempt: 4, .. }), "{d:?}");

    // Connected for exactly 60 s: stable, the count restarts at 1.
    let t1 = t0 + Duration::from_secs(100);
    p.on_connected(t1);
    let d = p.on_disconnect(&DisconnectReason::Network, t1 + Duration::from_secs(60));
    assert!(matches!(d, ReconnectDecision::Retry { attempt: 1, .. }), "{d:?}");
    assert!(matches!(d, ReconnectDecision::Retry { delay, .. } if delay <= Duration::from_millis(500)));

    // A disconnect without a new connect never resets.
    let d = p.on_disconnect(&DisconnectReason::Network, t1 + Duration::from_secs(600));
    assert!(matches!(d, ReconnectDecision::Retry { attempt: 2, .. }), "{d:?}");
}

#[test]
fn stable_reset_also_applies_before_a_non_retryable_reason() {
    let mut p = ReconnectPolicy::new(ReconnectConfig::default(), 3);
    let t0 = Instant::now();
    p.on_disconnect(&DisconnectReason::Network, t0);
    p.on_connected(t0);
    assert_eq!(
        p.on_disconnect(&DisconnectReason::UserClosed, t0 + Duration::from_secs(61)),
        ReconnectDecision::GiveUp(GiveUpReason::NotRetryable)
    );
    assert_eq!(p.attempts(), 0);
}

#[test]
fn offline_waits_without_consuming_attempts_then_retries_now() {
    let mut p =
        ReconnectPolicy::new(ReconnectConfig { max_attempts: Some(2), ..ReconnectConfig::default() }, 9);
    let t = Instant::now();
    assert!(matches!(
        p.on_disconnect(&DisconnectReason::Network, t),
        ReconnectDecision::Retry { attempt: 1, .. }
    ));
    p.set_network_reachable(false);
    assert!(!p.network_reachable());
    for _ in 0..50 {
        assert_eq!(
            p.on_disconnect(&DisconnectReason::Network, t),
            ReconnectDecision::WaitForNetwork { attempt: 2 }
        );
    }
    assert_eq!(p.retry_now(), ReconnectDecision::WaitForNetwork { attempt: 2 });
    p.set_network_reachable(true);
    assert_eq!(p.retry_now(), ReconnectDecision::Retry { attempt: 2, delay: Duration::ZERO });
    assert_eq!(p.retry_now(), ReconnectDecision::GiveUp(GiveUpReason::AttemptsExhausted));
}

#[test]
fn retry_now_does_not_disturb_the_jitter_sequence() {
    let mut p = ReconnectPolicy::new(unlimited(), 42);
    let t = Instant::now();
    let first = p.on_disconnect(&DisconnectReason::Network, t);
    assert_eq!(p.retry_now(), ReconnectDecision::Retry { attempt: 2, delay: Duration::ZERO });
    let third = p.on_disconnect(&DisconnectReason::Network, t);
    // Seed 42 table: 371 ms, then (second draw) 160 ms against the attempt-3 ceiling of 2000 ms.
    assert_eq!(first, ReconnectDecision::Retry { attempt: 1, delay: Duration::from_millis(371) });
    let ReconnectDecision::Retry { attempt: 3, delay } = third else { panic!("{third:?}") };
    assert!(delay <= Duration::from_millis(2000));
}

#[test]
fn reset_forgets_attempts() {
    let mut p = ReconnectPolicy::new(ReconnectConfig::default(), 5);
    let t = Instant::now();
    p.on_disconnect(&DisconnectReason::Network, t);
    p.on_disconnect(&DisconnectReason::Network, t);
    p.reset();
    assert_eq!(p.attempts(), 0);
    assert!(matches!(
        p.on_disconnect(&DisconnectReason::Network, t),
        ReconnectDecision::Retry { attempt: 1, .. }
    ));
}

proptest! {
    #[test]
    fn delays_stay_within_full_jitter_bounds(seed in any::<u64>(), n in 1usize..40) {
        let mut p = ReconnectPolicy::new(unlimited(), seed);
        let t = Instant::now();
        for i in 1..=n {
            let d = p.on_disconnect(&DisconnectReason::TlsEof, t);
            let ReconnectDecision::Retry { attempt, delay } = d else {
                return Err(TestCaseError::fail(format!("{d:?}")));
            };
            prop_assert_eq!(attempt as usize, i);
            let exp = u32::try_from(i - 1).unwrap().min(20);
            let ceiling = Duration::from_millis(500u64 << exp).min(Duration::from_secs(30));
            prop_assert!(delay <= ceiling, "attempt {} delay {:?} > {:?}", i, delay, ceiling);
            prop_assert_eq!(p.ceiling(attempt), ceiling);
        }
    }

    #[test]
    fn below_or_eq_is_bounded(seed in any::<u64>(), max in any::<u64>()) {
        let mut r = SplitMix64::new(seed);
        for _ in 0..16 {
            prop_assert!(r.below_or_eq(max) <= max);
        }
    }
}
