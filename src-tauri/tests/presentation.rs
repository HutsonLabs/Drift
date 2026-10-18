//! M1-6 / M6-2 / M3-2 Red: pure presentation rules for session windows.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::sync::Arc;
use std::time::Duration;

use drift_app::connections::{CONNECTIONS_WINDOW, ConnectionStatus};
use drift_app::present::{
    CloseAction, Focus, Hud, Surface, TITLEBAR_HEIGHT, accessibility_label, band_y, chrome, close_action,
    close_confirmation, close_needs_confirmation, cursor_shape, display_name, find_autoconnect, focus_for,
    greeter_hint, hud_frame, identity, quit_confirmation, session_item, status_for, surface_for,
    window_title,
};
use drift_app::view::{Screen, SessionView};
use drift_core::{
    ConnectMode, ConnectStage, ConnectionProfile, DesktopSize, DisconnectReason, Point, SessionState, Size,
};
use drift_macos::CursorShape;
use drift_rdp::{CursorBitmap, CursorUpdate, SessionEvent};

fn view_in(mode: ConnectMode, state: SessionState) -> SessionView {
    let mut p = ConnectionProfile::new("Homelab", "10.1.2.40", mode);
    p.linux_username = Some("drifttest".into());
    let mut v = SessionView::new(&p);
    v.apply(&SessionEvent::State(state));
    v
}

/// A view forced onto `screen` (the fields the surface rules read are set directly).
fn on_screen(screen: Screen) -> SessionView {
    let mut v = SessionView::new(&ConnectionProfile::new("Homelab", "h", ConnectMode::RemoteLogin));
    v.screen = screen;
    v
}

#[test]
fn webview_is_hidden_only_while_there_is_a_live_picture() {
    #[rustfmt::skip]
    let table = [
        (Screen::Profiles, Surface::Webview),
        (Screen::Connecting, Surface::Webview),
        (Screen::Certificate, Surface::Webview),
        (Screen::Error, Surface::Webview),
        (Screen::Live, Surface::Remote),
        // M7-3: the last frame stays on screen, dimmed under a transparent full-window overlay.
        (Screen::Reconnecting, Surface::Overlay),
        // M3-2/M7-3: a non-modal banner over the greeter picture; the RemoteView keeps focus.
        (Screen::GreeterHint, Surface::Hud(Hud::Banner)),
    ];
    for (screen, surface) in table {
        assert_eq!(surface_for(&on_screen(screen)), surface, "{screen:?}");
    }
}

#[test]
fn the_statistics_hud_floats_over_the_live_picture() {
    // M1 "Done (manual M1)": `anim.py` shows >= 55 fps in the stats overlay.
    let mut live = on_screen(Screen::Live);
    live.stats = Some(Default::default());
    assert_eq!(surface_for(&live), Surface::Remote, "off by default");
    live.show_stats = true;
    assert_eq!(surface_for(&live), Surface::Hud(Hud::Stats));
    live.stats = None;
    assert_eq!(surface_for(&live), Surface::Remote, "nothing to draw before the first sample");
    live.stats = Some(Default::default());
    // The HUD is only a HUD while a picture is live; elsewhere the full webview wins.
    let mut form = on_screen(Screen::Profiles);
    form.show_stats = true;
    assert_eq!(surface_for(&form), Surface::Webview);
    let mut retry = on_screen(Screen::Reconnecting);
    retry.show_stats = true;
    assert_eq!(surface_for(&retry), Surface::Overlay);
}

#[test]
fn a_hud_never_covers_the_whole_picture() {
    let parent = Size::new(1280.0_f64, 800.0);
    for hud in [Hud::Banner, Hud::Stats] {
        let f = hud_frame(parent, hud, false);
        assert!(f.x >= 0.0 && f.y >= 0.0, "{hud:?} {f:?}");
        assert!(f.x + f.width <= parent.width && f.y + f.height <= parent.height, "{hud:?} {f:?}");
        let covered = f.width * f.height / (parent.width * parent.height);
        assert!(covered < 0.1, "{hud:?} covers {covered:.3} of the picture");
    }
    // The banner sits at the top centre, the statistics panel in the bottom-right corner.
    let banner = hud_frame(parent, Hud::Banner, true);
    assert_eq!(banner.y, 0.0, "banner at the top of a flipped superview");
    assert!((banner.x - (parent.width - banner.width) / 2.0).abs() < 0.5, "centred: {banner:?}");
    assert!(banner.flexible.left && banner.flexible.right && banner.flexible.bottom);
    assert!(!banner.flexible.top, "the banner stays glued to the top edge");
    let stats = hud_frame(parent, Hud::Stats, true);
    assert!(stats.x + stats.width < parent.width, "inset from the right");
    assert!(stats.y > parent.height / 2.0, "bottom half: {stats:?}");
    assert!(stats.flexible.left && stats.flexible.top);
    assert!(!stats.flexible.right && !stats.flexible.bottom, "glued to the bottom-right corner");
}

#[test]
fn hud_geometry_follows_the_superviews_flippedness() {
    let parent = Size::new(1280.0_f64, 800.0);
    for hud in [Hud::Banner, Hud::Stats] {
        let flipped = hud_frame(parent, hud, true);
        let appkit = hud_frame(parent, hud, false);
        assert_eq!((flipped.width, flipped.height), (appkit.width, appkit.height), "{hud:?}");
        assert_eq!(flipped.x, appkit.x, "{hud:?}");
        // AppKit's origin is bottom-left: the same panel is mirrored vertically.
        assert!(
            (appkit.y - (parent.height - flipped.y - flipped.height)).abs() < f64::EPSILON,
            "{hud:?}: flipped {flipped:?} vs appkit {appkit:?}"
        );
    }
}

#[test]
fn a_hud_shrinks_with_a_narrow_window() {
    let narrow = Size::new(320.0_f64, 240.0);
    for hud in [Hud::Banner, Hud::Stats] {
        let f = hud_frame(narrow, hud, false);
        assert!(f.x >= 0.0 && f.y >= 0.0 && f.width > 0.0 && f.height > 0.0, "{hud:?} {f:?}");
        assert!(f.x + f.width <= narrow.width, "{hud:?} {f:?}");
        assert!(f.y + f.height <= narrow.height, "{hud:?} {f:?}");
    }
}

/// UI-windows decision 5: the window title is the plain profile name, so Mission Control,
/// Cmd+` and the Window menu show it.
#[test]
fn the_window_title_is_the_profile_name() {
    let states = [
        SessionState::Idle,
        SessionState::Connecting { leg: 2, stage: ConnectStage::Tls },
        SessionState::AwaitingGreeterLogin,
        SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 },
        SessionState::Failed { reason: DisconnectReason::AuthFailed },
    ];
    for state in states {
        assert_eq!(window_title(&view_in(ConnectMode::Headless, state.clone())), "Homelab", "{state:?}");
    }
    let mut odd = SessionView::new(&ConnectionProfile::new("  Work\tbox ", "h", ConnectMode::Headless));
    assert_eq!(window_title(&odd), "Work box");
    odd.profile_name = "   ".into();
    assert_eq!(window_title(&odd), "Untitled");
    assert_eq!(display_name("a\nb"), "a b");
}

fn reconnecting() -> SessionState {
    SessionState::Reconnecting {
        attempt: 1,
        next_in: Duration::from_secs(1),
        reason: DisconnectReason::Network,
    }
}

/// UI-windows decision 6 (UI-tabs decision 5's table): status for every screen.
#[test]
fn status_and_identity_follow_the_screen() {
    #[rustfmt::skip]
    let table = [
        (Screen::Profiles, ConnectionStatus::Connecting),
        (Screen::Connecting, ConnectionStatus::Connecting),
        (Screen::Certificate, ConnectionStatus::Connecting),
        (Screen::Live, ConnectionStatus::Live),
        (Screen::GreeterHint, ConnectionStatus::Live),
        (Screen::Reconnecting, ConnectionStatus::Reconnecting),
        (Screen::Error, ConnectionStatus::Failed),
    ];
    for (screen, status) in table {
        assert_eq!(status_for(&on_screen(screen)), status, "{screen:?}");
    }
    let mut profile = ConnectionProfile::new("Homelab", "10.1.2.40", ConnectMode::RemoteLogin);
    profile.linux_username = Some("drifttest".into());
    let mut greeter = SessionView::new(&profile);
    greeter.apply(&SessionEvent::State(SessionState::AwaitingGreeterLogin));
    greeter.show_stats = true;
    let id = identity(&profile, &greeter);
    assert_eq!(id.profile_id, profile.id);
    assert_eq!(
        (id.name.as_str(), id.host.as_str(), id.mode),
        ("Homelab", "10.1.2.40", ConnectMode::RemoteLogin)
    );
    assert_eq!(id.status, ConnectionStatus::Live);
    assert_eq!(id.hint.as_deref(), Some("Log in as “drifttest” to start your session"), "tooltip");
    assert!(id.show_stats, "gauge pressed");
    let live = view_in(
        ConnectMode::RemoteLogin,
        SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 },
    );
    assert_eq!(identity(&profile, &live).hint, None);
}

/// The Window and Dock menus: state glyph (UI-tabs decision 11) and cleaned name.
#[test]
fn session_items_carry_a_glyph_and_the_name() {
    #[rustfmt::skip]
    let table = [
        (SessionState::Connecting { leg: 1, stage: ConnectStage::Tcp }, '◌', ConnectionStatus::Connecting),
        (SessionState::AwaitingGreeterLogin, '◐', ConnectionStatus::Live),
        (SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 }, '●', ConnectionStatus::Live),
        (reconnecting(), '↻', ConnectionStatus::Reconnecting),
        (SessionState::Failed { reason: DisconnectReason::AuthFailed }, '⚠', ConnectionStatus::Failed),
        (SessionState::Disconnected { reason: DisconnectReason::ServerShutdown }, '⚠', ConnectionStatus::Failed),
    ];
    for (state, glyph, status) in table {
        let item = session_item("session-3", &view_in(ConnectMode::Headless, state.clone()));
        assert_eq!(item.window, "session-3");
        assert_eq!((item.glyph, item.status, item.name.as_str()), (glyph, status, "Homelab"), "{state:?}");
        assert_eq!(item.title(), format!("{glyph} Homelab"));
    }
}

/// UI-windows decision 4: only a window that holds a desktop asks before closing.
#[test]
fn closing_asks_only_while_a_desktop_is_held() {
    #[rustfmt::skip]
    let table = [
        (Screen::Profiles, false),
        (Screen::Connecting, false),
        (Screen::Certificate, false),
        (Screen::Error, false),
        (Screen::Live, true),
        (Screen::GreeterHint, true),
        (Screen::Reconnecting, true),
    ];
    for (screen, ask) in table {
        assert_eq!(close_needs_confirmation(&on_screen(screen)), ask, "{screen:?}");
    }
}

/// UI-windows decisions 2 and 4: the Connections window only hides; a session window asks
/// first while live, else closes at once.
#[test]
fn the_close_button_hides_connections_and_confirms_live_sessions() {
    assert_eq!(CONNECTIONS_WINDOW, "connections");
    assert_eq!(close_action(CONNECTIONS_WINDOW, None), CloseAction::Hide);
    assert_eq!(close_action(CONNECTIONS_WINDOW, Some(&on_screen(Screen::Live))), CloseAction::Hide);
    assert_eq!(close_action("session-4", Some(&on_screen(Screen::Live))), CloseAction::Confirm);
    assert_eq!(close_action("session-4", Some(&on_screen(Screen::Reconnecting))), CloseAction::Confirm);
    assert_eq!(close_action("session-4", Some(&on_screen(Screen::Connecting))), CloseAction::CloseNow);
    assert_eq!(close_action("session-4", Some(&on_screen(Screen::Error))), CloseAction::CloseNow);
    assert_eq!(close_action("session-4", None), CloseAction::CloseNow, "no session any more");
}

#[test]
fn confirmation_texts() {
    let c = close_confirmation(" Homelab ");
    assert_eq!(c.message, "Disconnect “Homelab”?");
    assert_eq!(c.informative, "The remote session keeps running on the host.");
    assert_eq!((c.confirm.as_str(), c.cancel.as_str()), ("Disconnect", "Cancel"));
    assert_eq!(quit_confirmation(0), None, "no sessions: quit at once");
    let one = quit_confirmation(1).unwrap();
    assert_eq!(one.message, "Quit Drift?");
    assert_eq!(one.informative, "1 session will be disconnected.");
    assert_eq!((one.confirm.as_str(), one.cancel.as_str()), ("Quit", "Cancel"));
    assert_eq!(quit_confirmation(3).unwrap().informative, "3 sessions will be disconnected.");
}

/// UI-tabs decision 10 / UI-windows: the greeter hint is the floating banner and the identity
/// capsule's tooltip (no window subtitle).
#[test]
fn the_greeter_hint_names_the_linux_user() {
    assert_eq!(greeter_hint(None), None);
    let live = view_in(
        ConnectMode::RemoteLogin,
        SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 },
    );
    assert_eq!(greeter_hint(Some(&live)), None);
    let greeter = view_in(ConnectMode::RemoteLogin, SessionState::AwaitingGreeterLogin);
    assert_eq!(greeter_hint(Some(&greeter)).as_deref(), Some("Log in as “drifttest” to start your session"));
    let mut resuming = live.clone();
    resuming.apply(&SessionEvent::State(SessionState::AwaitingGreeterLogin));
    assert!(resuming.resuming);
    assert_eq!(
        greeter_hint(Some(&resuming)).as_deref(),
        Some("Session is still running — log in as “drifttest” to resume")
    );
    let mut anon = ConnectionProfile::new("Homelab", "h", ConnectMode::RemoteLogin);
    anon.linux_username = None;
    let mut v = SessionView::new(&anon);
    v.apply(&SessionEvent::State(SessionState::Connecting { leg: 1, stage: ConnectStage::Tcp }));
    v.apply(&SessionEvent::State(SessionState::AwaitingGreeterLogin));
    assert_eq!(greeter_hint(Some(&v)).as_deref(), Some("Log in to start your session"));
}

/// M9-4: VoiceOver names the picture after the connection and the desktop it shows.
#[test]
fn the_picture_is_labelled_for_voiceover() {
    assert_eq!(accessibility_label(None), "Remote desktop");
    let live = view_in(
        ConnectMode::Headless,
        SessionState::Connected { desktop: DesktopSize::new(2560, 1600), scale: 200 },
    );
    assert_eq!(accessibility_label(Some(&live)), "Homelab — remote desktop, 2560 by 1600 pixels");
    let greeter = view_in(ConnectMode::RemoteLogin, SessionState::AwaitingGreeterLogin);
    assert_eq!(accessibility_label(Some(&greeter)), "Homelab — remote desktop");
}

#[test]
fn cursor_updates_map_to_shapes() {
    assert_eq!(cursor_shape(&CursorUpdate::Hidden), Some((CursorShape::Hidden, 100)));
    assert_eq!(cursor_shape(&CursorUpdate::Default), Some((CursorShape::Default, 100)));
    assert_eq!(cursor_shape(&CursorUpdate::Position(Point::new(3, 4))), None);
    let bgra: Arc<[u8]> = vec![1u8; 86 * 86 * 4].into();
    let b =
        CursorBitmap { size: Size::new(86, 86), hotspot: Point::new(5, 6), bgra: bgra.clone(), scale: 200 };
    let Some((CursorShape::Image(img), scale)) = cursor_shape(&CursorUpdate::Bitmap(b)) else {
        panic!("bitmap → image");
    };
    assert_eq!(scale, 200);
    assert_eq!(img.size, Size::new(86, 86));
    assert_eq!(img.hotspot, Point::new(5, 6));
    assert_eq!(img.bgra, bgra);
    assert_eq!(img.size_points(scale), Size::new(43.0, 43.0));
}

#[test]
fn autoconnect_finds_the_named_profile() {
    let a = ConnectionProfile::new("Homelab", "h", ConnectMode::Headless);
    let b = ConnectionProfile::new("homelab login", "h", ConnectMode::RemoteLogin);
    let c = ConnectionProfile::new("HOMELAB LOGIN", "h", ConnectMode::RemoteLogin);
    let all = [a.clone(), b.clone(), c.clone()];
    assert_eq!(find_autoconnect(&all, "Homelab").map(|p| p.id), Some(a.id));
    assert_eq!(find_autoconnect(&all, "homelab").map(|p| p.id), Some(a.id), "unique case-insensitive");
    assert_eq!(find_autoconnect(&all, "HOMELAB LOGIN").map(|p| p.id), Some(c.id), "exact wins");
    assert_eq!(find_autoconnect(&all, "Homelab Login"), None, "ambiguous");
    assert_eq!(find_autoconnect(&all, "  Homelab ").map(|p| p.id), Some(a.id), "trimmed");
    assert_eq!(find_autoconnect(&all, "nope"), None);
    assert_eq!(find_autoconnect(&all, ""), None);
}

/// UI-windows decision 5: a 52-point transparent title bar, no strip; the picture and the page
/// start right under it.
#[test]
fn the_picture_starts_under_the_52_point_title_bar() {
    assert!((TITLEBAR_HEIGHT - 52.0).abs() < f64::EPSILON);
    let c = chrome(false);
    assert_eq!((c.titlebar, c.content_top), (TITLEBAR_HEIGHT, TITLEBAR_HEIGHT));
}

/// UI-windows decision 5 / board 6: in full screen nothing draws over the picture.
#[test]
fn full_screen_has_no_chrome_and_the_picture_fills_the_screen() {
    let c = chrome(true);
    assert_eq!((c.titlebar, c.content_top), (0.0, 0.0));
}

#[test]
fn bands_are_measured_from_the_top_edge_in_either_coordinate_space() {
    // The 52-point title bar at the top of an 800-point view.
    assert!((band_y(800.0, 0.0, 52.0, true) - 0.0).abs() < f64::EPSILON);
    assert!((band_y(800.0, 0.0, 52.0, false) - 748.0).abs() < f64::EPSILON);
    // The content below it.
    assert!((band_y(800.0, 52.0, 748.0, true) - 52.0).abs() < f64::EPSILON);
    assert!((band_y(800.0, 52.0, 748.0, false) - 0.0).abs() < f64::EPSILON);
}

/// Clicking the title bar must not leave the keyboard in its webview: focus goes back to
/// whatever the window's surface says owns it (UI-windows decision 11).
#[test]
fn keyboard_focus_follows_the_surface() {
    assert_eq!(focus_for(Surface::Remote), Focus::Remote);
    assert_eq!(focus_for(Surface::Hud(Hud::Banner)), Focus::Remote);
    assert_eq!(focus_for(Surface::Hud(Hud::Stats)), Focus::Remote);
    assert_eq!(focus_for(Surface::Webview), Focus::Page);
    assert_eq!(focus_for(Surface::Overlay), Focus::Page);
}
