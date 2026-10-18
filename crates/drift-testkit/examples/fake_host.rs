//! A loopback RDP host for running the app by hand (IOS-5 first light in the iOS simulator,
//! which shares the Mac's `127.0.0.1`).
//!
//! It serves [`FakeServer`] NLA legs with the graphics, Display Control and clipboard channels,
//! and draws a moving square at about 30 fps for ten minutes per connection, so a live picture,
//! resizes and reconnects can be checked without the GNOME test host. Every connection gets a
//! fresh leg, up to [`LEGS`].
//!
//! ```text
//! cargo run -p drift-testkit --example fake_host -- <config dir>
//! ```
//!
//! It writes `<config dir>/profiles.toml` with one Headless profile ("Fake host") pinned to the
//! server's certificate and prints the profile id and the test-only password for
//! `DRIFT_SMOKE_SECRET`. Test values only: nothing here is a real credential.
//!
//! With `FAKE_HOST_CLOSE_AFTER=<seconds>`, every connection is closed by the host after that
//! long, which sends the client through its reconnect path (M7, IOS-5).
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, ProfileStore};
use drift_testkit::Channels;
use drift_testkit::fake_server::{FakeServer, LegScript, ServerAction, TestCert};

/// Connections served before the host stops answering.
const LEGS: usize = 20;
/// Test-only user name and password (not a credential).
const USER: &str = "drift";
const PASSWORD: &str = "fake-host-password";
const FRAME: Duration = Duration::from_millis(33);
const FRAMES: u32 = 30 * 600;
const SIZE: (u16, u16) = (1280, 800);
const SQUARE: u16 = 120;

fn leg(cert: &TestCert, close_after: Option<u32>) -> LegScript {
    let mut leg = LegScript::nla(cert.clone(), USER, PASSWORD)
        .with_channels(Channels::all())
        .then(ServerAction::GfxReset(u32::from(SIZE.0), u32::from(SIZE.1)));
    let (mut x, mut y, mut dx, mut dy) = (0_i32, 0_i32, 9_i32, 6_i32);
    let frames = close_after.map_or(FRAMES, |secs| secs * 30);
    for frame in 0..frames {
        let old = rect(x, y);
        x += dx;
        y += dy;
        if x < 0 || x + i32::from(SQUARE) > i32::from(SIZE.0) {
            dx = -dx;
            x += 2 * dx;
        }
        if y < 0 || y + i32::from(SQUARE) > i32::from(SIZE.1) {
            dy = -dy;
            y += 2 * dy;
        }
        let shade = u8::try_from(frame % 200).unwrap_or(0) + 40;
        leg = leg
            .then(ServerAction::GfxFill { rect: old, color: [0x60, 0x40, 0x20] })
            .then(ServerAction::GfxFill { rect: rect(x, y), color: [0xF0, shade, 0x50] })
            .then(ServerAction::Wait(FRAME));
    }
    if close_after.is_some() {
        leg = leg.then(ServerAction::Close);
    }
    leg
}

fn rect(x: i32, y: i32) -> (u16, u16, u16, u16) {
    let x = u16::try_from(x.max(0)).unwrap_or(0);
    let y = u16::try_from(y.max(0)).unwrap_or(0);
    (x, y, x + SQUARE, y + SQUARE)
}

#[tokio::main]
async fn main() {
    let dir = std::env::args().nth(1).expect("usage: fake_host <config dir>");
    let cert = TestCert::generate("fake-host.local");
    let close_after = std::env::var("FAKE_HOST_CLOSE_AFTER").ok().and_then(|s| s.parse().ok());
    let script = leg(&cert, close_after);
    let server = FakeServer::start(vec![script; LEGS]).await.expect("listen on 127.0.0.1");

    let mut profile = ConnectionProfile::new("Fake host", "127.0.0.1", ConnectMode::Headless);
    profile.port = server.port();
    USER.clone_into(&mut profile.rdp_username);
    profile.cert_pin = Some(cert.fingerprint());
    let mut store = ProfileStore::new();
    store.upsert(profile.clone()).expect("a valid profile");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(std::path::Path::new(&dir).join("profiles.toml"), store.to_toml().expect("toml"))
        .expect("write profiles.toml");

    println!("fake host listening on {}", server.addr());
    println!("DRIFT_AUTOCONNECT=Fake host");
    println!("DRIFT_SMOKE_SECRET={}:rdp-user:{PASSWORD}", profile.id);
    tokio::signal::ctrl_c().await.ok();
    for (i, leg) in server.log().legs.iter().enumerate() {
        println!("leg {i}: error={:?}", leg.error);
    }
}
