//! HOST-10 Red: the private `CGVirtualDisplay` API is present on this macOS (a self-test that
//! creates nothing), and — by hand only — a virtual display appears and disappears.
#![cfg(target_os = "macos")]
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_host::cg_virtual;

#[test]
fn the_self_test_finds_the_private_api_without_creating_a_display() {
    match cg_virtual::self_test() {
        Ok(()) => {}
        Err(e) => panic!("CGVirtualDisplay is missing on this macOS: {e}"),
    }
}

/// Changes the display arrangement for a moment: run by hand
/// (`cargo test -p drift-host --test cg_virtual -- --ignored`).
#[test]
#[ignore = "creates a real (virtual) display for a moment"]
fn creates_and_removes_a_virtual_display() {
    use drift_host::display::LayoutRequest;
    let plan = drift_host::vdisplay_plan::plan(&LayoutRequest {
        pixels: drift_core::Size::new(1600, 1000),
        desktop_scale: Some(200),
        physical_mm: None,
    });
    let display = cg_virtual::VirtualDisplay::create("Drift Host test", &plan).unwrap();
    let id = display.display_id();
    assert!(cg_virtual::active_displays().contains(&id), "{id} not active");
    drop(display);
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(!cg_virtual::active_displays().contains(&id), "{id} still active");
}
