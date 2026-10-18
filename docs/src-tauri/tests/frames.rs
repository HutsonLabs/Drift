//! UI-windows Red: per-profile window frames (ADR UI-windows-gallery decision 7).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use drift_app::frames::{CONNECTIONS_KEY, FRAMES_FILE, Frame, FrameStore, Template, profile_key, restore};
use uuid::Uuid;

const SESSION: Template = Template { width: 1280.0, height: 800.0, min_width: 480.0, min_height: 320.0 };

fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
    Frame { x, y, width, height }
}

/// One 1728×1117 laptop screen with a 2560×1440 monitor to its right.
fn screens() -> Vec<Frame> {
    vec![frame(0.0, 0.0, 1728.0, 1117.0), frame(1728.0, -200.0, 2560.0, 1440.0)]
}

#[test]
fn frames_round_trip_through_json_next_to_profiles() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(FRAMES_FILE, "window-frames.json");
    let id = Uuid::new_v4();
    {
        let store = FrameStore::in_dir(dir.path());
        assert_eq!(store.get(&profile_key(id)), None, "nothing saved yet");
        store.set(&profile_key(id), frame(100.0, 80.0, 1400.0, 900.0)).unwrap();
        store.set(CONNECTIONS_KEY, frame(10.0, 20.0, 1100.0, 720.0)).unwrap();
    }
    let path = dir.path().join(FRAMES_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json[profile_key(id)]["width"], 1400.0, "{text}");
    assert_eq!(json[CONNECTIONS_KEY]["x"], 10.0, "{text}");
    assert!(!text.contains("name") && !text.contains("host"), "only geometry is written: {text}");

    let store = FrameStore::in_dir(dir.path());
    assert_eq!(store.get(&profile_key(id)), Some(frame(100.0, 80.0, 1400.0, 900.0)));
    assert_eq!(store.get(CONNECTIONS_KEY), Some(frame(10.0, 20.0, 1100.0, 720.0)));

    // Deleting a profile deletes its frame.
    store.remove(&profile_key(id)).unwrap();
    assert_eq!(FrameStore::in_dir(dir.path()).get(&profile_key(id)), None);
    assert!(FrameStore::in_dir(dir.path()).get(CONNECTIONS_KEY).is_some());
}

#[test]
fn a_damaged_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FRAMES_FILE), "{ not json").unwrap();
    let store = FrameStore::in_dir(dir.path());
    assert_eq!(store.get(CONNECTIONS_KEY), None);
    store.set(CONNECTIONS_KEY, frame(1.0, 2.0, 600.0, 400.0)).unwrap();
    assert_eq!(FrameStore::in_dir(dir.path()).get(CONNECTIONS_KEY), Some(frame(1.0, 2.0, 600.0, 400.0)));
}

#[test]
fn a_saved_frame_on_a_current_screen_is_kept() {
    let saved = frame(200.0, 100.0, 1000.0, 700.0);
    assert_eq!(restore(Some(saved), &screens(), SESSION, 0), saved);
    // On the second monitor, partly off its top: still enough of it is visible.
    let saved = frame(3000.0, -150.0, 1300.0, 900.0);
    assert_eq!(restore(Some(saved), &screens(), SESSION, 3), saved, "cascade applies to defaults only");
    // Only a 64×64 corner visible is enough.
    let saved = frame(1728.0 - 64.0, 1117.0 - 64.0, 800.0, 600.0);
    assert_eq!(restore(Some(saved), &screens()[..1], SESSION, 0), saved);
}

#[test]
fn off_screen_or_too_small_frames_fall_back_to_the_centred_template() {
    let centred = frame((1728.0 - 1280.0) / 2.0, (1117.0 - 800.0) / 2.0, 1280.0, 800.0);
    // The monitor it was on is unplugged.
    assert_eq!(restore(Some(frame(3000.0, 100.0, 1300.0, 900.0)), &screens()[..1], SESSION, 0), centred);
    // Less than 64×64 on screen.
    assert_eq!(
        restore(Some(frame(1728.0 - 63.0, 100.0, 800.0, 600.0)), &screens()[..1], SESSION, 0),
        centred
    );
    // Smaller than the template's minimum.
    assert_eq!(restore(Some(frame(100.0, 100.0, 300.0, 600.0)), &screens(), SESSION, 0), centred);
    assert_eq!(restore(None, &screens(), SESSION, 0), centred);
    // Cascaded by 24 pt per open session window.
    let second = restore(None, &screens(), SESSION, 2);
    assert_eq!(second, frame(centred.x + 48.0, centred.y + 48.0, 1280.0, 800.0));
}

#[test]
fn restore_without_screens_uses_the_template_size() {
    let f = restore(None, &[], SESSION, 0);
    assert_eq!((f.width, f.height), (1280.0, 800.0));
}
