//! HOST-12 Red: the lossy link used to measure transports drops packets deterministically.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_transport::lossy::LossModel;

#[test]
fn zero_loss_never_drops() {
    let mut m = LossModel::new(0.0, 1);
    assert!((0..10_000).all(|_| !m.drop_next()));
}

#[test]
fn the_drop_rate_matches_the_configured_loss() {
    for percent in [1.0, 3.0, 5.0, 20.0] {
        let mut m = LossModel::new(percent, 42);
        let dropped = (0..100_000).filter(|_| m.drop_next()).count();
        let rate = dropped as f64 / 1000.0;
        assert!((rate - percent).abs() < 0.5, "{percent}% configured, {rate}% dropped");
    }
}

#[test]
fn the_same_seed_drops_the_same_packets() {
    let run = |seed| {
        let mut m = LossModel::new(5.0, seed);
        (0..1000).map(|_| m.drop_next()).collect::<Vec<_>>()
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}
