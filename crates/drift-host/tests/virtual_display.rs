//! HOST-10 Red: client layouts become virtual-display modes (pure planning), the display manager
//! is told about every layout, and without a virtual display the host keeps the physical one.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_core::Size;
use drift_host::display::{DisplayManager, DisplayTarget, LayoutRequest, PhysicalOnly};
use drift_host::vdisplay_plan::{MAX_PIXELS, ModePlan, plan};

fn req(w: u32, h: u32, scale: Option<u32>) -> LayoutRequest {
    LayoutRequest { pixels: Size::new(w, h), desktop_scale: scale, physical_mm: None }
}

#[test]
fn a_retina_ipad_gets_a_hidpi_mode_at_its_native_pixels() {
    let p = plan(&req(2732, 2048, Some(200)));
    assert_eq!(p.pixels, Size::new(2732, 2048));
    assert!(p.hidpi);
    assert_eq!(p.points, Size::new(1366, 1024));
    assert_eq!(p.refresh_hz, 60);
}

#[test]
fn a_100_percent_client_gets_a_plain_mode() {
    let p = plan(&req(1920, 1080, Some(100)));
    assert!(!p.hidpi);
    assert_eq!(p.points, p.pixels);
    let p = plan(&req(1920, 1080, None));
    assert!(!p.hidpi, "no scale means 100 %");
}

#[test]
fn odd_and_tiny_sizes_become_even_and_at_least_the_minimum() {
    assert_eq!(plan(&req(1001, 701, None)).pixels, Size::new(1000, 700));
    assert_eq!(plan(&req(10, 10, None)).pixels, Size::new(640, 480));
    // HiDPI needs whole points.
    let p = plan(&req(1002, 702, Some(200)));
    assert_eq!(p.points, Size::new(501, 351));
}

#[test]
fn huge_layouts_shrink_to_the_maximum_and_keep_their_shape() {
    assert_eq!(MAX_PIXELS, Size::new(5120, 2880));
    let p = plan(&req(7680, 4320, Some(200)));
    assert_eq!(p.pixels, Size::new(5120, 2880));
    let p = plan(&req(8000, 2000, None));
    assert_eq!(p.pixels, Size::new(5120, 1280));
    assert!(p.pixels.width % 2 == 0 && p.pixels.height % 2 == 0);
}

#[test]
fn physical_size_comes_from_the_client_or_a_typical_density() {
    let mut r = req(2732, 2048, Some(200));
    r.physical_mm = Some((280, 210));
    assert_eq!(plan(&r).size_mm, (280, 210));
    // 220 pixels per inch for Retina, 110 otherwise.
    assert_eq!(plan(&req(2200, 2200, Some(200))).size_mm, (254, 254));
    assert_eq!(plan(&req(1100, 1100, None)).size_mm, (254, 254));
}

#[test]
fn only_a_different_mode_needs_a_change() {
    let a = plan(&req(1920, 1080, None));
    assert!(!ModePlan::needs_change(Some(&a), &a));
    assert!(ModePlan::needs_change(None, &a));
    assert!(ModePlan::needs_change(Some(&a), &plan(&req(1920, 1200, None))));
}

#[test]
fn the_physical_only_manager_always_answers_physical() {
    let mut m = PhysicalOnly;
    assert_eq!(m.apply(&req(2732, 2048, Some(200))), DisplayTarget::Physical);
}
