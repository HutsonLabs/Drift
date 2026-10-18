//! A loopback Drift Host that mirrors this Mac's menu bar, for trying EXT-3 by hand (an iPad
//! simulator shares the Mac's `127.0.0.1`).
//!
//! ```text
//! cargo run -p drift-host --example menu_host -- <config dir>
//! ```
//!
//! It serves the test pattern without pairing, offers the frontmost app's menus through
//! Accessibility, writes `<config dir>/profiles.toml` with one profile pinned to its certificate,
//! and prints the `DRIFT_SMOKE_SECRET` for it. Test values only: nothing here is a real
//! credential. Without Accessibility the client gets no menus (and the log says so).
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use drift_core::{ConnectMode, ConnectionProfile, ProfileStore, Size};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::menus::MenuSource;
use drift_host::menus::ax::AxMenus;
use drift_host::server::{Host, HostOptions};

const USER: &str = "drift";
const PASSWORD: &str = "menu-host-password";

fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let dir = std::env::args().nth(1).expect("usage: menu_host <config dir>");
    let identity_dir = tempfile::tempdir().expect("temp dir");
    let identity = HostIdentity::load_or_create(identity_dir.path()).expect("identity");
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASSWORD.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(RecordingInput::default()),
        clipboard: None,
        audio: None,
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing: None,
        menus: Some(AxMenus::start().expect("menu reader") as Arc<dyn MenuSource>),
    })
    .expect("listen on 127.0.0.1");

    let mut profile = ConnectionProfile::new("Menu host", "127.0.0.1", ConnectMode::Headless);
    profile.port = host.port();
    USER.clone_into(&mut profile.rdp_username);
    profile.cert_pin = Some(identity.fingerprint());
    let mut store = ProfileStore::new();
    store.upsert(profile.clone()).expect("a valid profile");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(std::path::Path::new(&dir).join("profiles.toml"), store.to_toml().expect("toml"))
        .expect("write profiles.toml");
    println!("menu host listening on 127.0.0.1:{}", host.port());
    println!("DRIFT_AUTOCONNECT=Menu host");
    println!("DRIFT_SMOKE_SECRET={}:rdp-user:{PASSWORD}", profile.id);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        println!("{:?}", host.stats().drift_mac);
    }
}
