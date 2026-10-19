//! UI-kinds Red: a profile's target kind (docs/design/mockup-targets.html). A profile paired with
//! a Drift Host is a Mac; every other profile is a GNOME computer.

use drift_core::{ConnectMode, ConnectionProfile, HostKind};
use uuid::Uuid;

#[test]
fn a_paired_profile_is_a_mac_and_any_other_is_gnome() {
    let mut p = ConnectionProfile::new("Mac mini", "mac-mini.local", ConnectMode::Headless);
    assert_eq!(p.kind(), HostKind::Gnome);
    p.paired_host = Some(Uuid::from_u128(7));
    assert_eq!(p.kind(), HostKind::Mac);
}
