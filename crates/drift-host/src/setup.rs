//! What the binary plugs in at start: the screen if it can be captured, injection if allowed
//! (humble wiring; the decisions are [`inject_enabled`]'s and the permission checks').

use drift_capture::config::{CaptureConfig, CaptureFormat};
use drift_capture::display::{DisplayChoice, DisplayInfo};

use crate::capture::{ScreenFrames, display_map};
use crate::frames::FrameError;
use crate::inject::InjectingInput;
use crate::input::{InputSink, RecordingInput};
use crate::pipeline::{LiveAvc444Encoder, LiveEncoder, LiveHevcEncoder, LiveRequest, LiveSource};
use crate::server::LiveFactory;

/// Frames a second the screen is captured and encoded at.
pub const LIVE_FPS: u32 = 60;

/// Whether to inject: the setting when given, otherwise whether the host has Accessibility.
pub const fn inject_enabled(setting: Option<bool>, accessibility: bool) -> bool {
    match setting {
        Some(on) => on,
        None => accessibility,
    }
}

/// The screen as the live picture, with the display it shows; `None` without Screen Recording.
pub fn capture_or_pattern() -> Option<(LiveFactory, DisplayInfo)> {
    if !drift_capture::sck::has_permission() {
        tracing::warn!(
            "no Screen Recording permission: showing a test pattern (System Settings › Privacy & Security › Screen Recording)"
        );
        return None;
    }
    let display = drift_capture::sck::main_display();
    // The request names the display (HOST-10's virtual one, or the main one) and the codec:
    // AVC444 and HEVC 4:4:4 are encoded from BGRA, AVC420 and HEVC 4:2:0 straight from NV12.
    let factory: LiveFactory = Box::new(|request: LiveRequest| {
        let config = CaptureConfig {
            max_fps: LIVE_FPS,
            display: request.display.map_or(DisplayChoice::Main, DisplayChoice::Id),
            format: if request.avc444 || request.hevc.is_some_and(|h| h.profile.full_chroma()) {
                CaptureFormat::Bgra
            } else {
                CaptureFormat::Nv12FullRange
            },
            ..CaptureConfig::default()
        };
        let screen = ScreenFrames::start(config).map_err(|e| FrameError(e.to_string()))?;
        if let Some(selection) = request.hevc {
            LiveHevcEncoder::new(screen, LIVE_FPS, selection).map(|e| Box::new(e) as Box<dyn LiveSource>)
        } else if request.avc444 {
            LiveAvc444Encoder::new(screen, LIVE_FPS).map(|e| Box::new(e) as Box<dyn LiveSource>)
        } else {
            LiveEncoder::new(screen, LIVE_FPS).map(|e| Box::new(e) as Box<dyn LiveSource>)
        }
    });
    Some((factory, display))
}

/// Where client input goes: injected into `display` when enabled, otherwise counted and logged.
pub fn input_for(setting: Option<bool>, display: Option<DisplayInfo>) -> Box<dyn InputSink> {
    let trusted = drift_inject::cg::accessibility_trusted();
    if !inject_enabled(setting, trusted) {
        tracing::info!("input is logged, not injected");
        return Box::new(RecordingInput::default());
    }
    if !trusted {
        tracing::warn!(
            "inject_input is on but Drift Host has no Accessibility permission: macOS will drop the events (System Settings › Privacy & Security › Accessibility)"
        );
    }
    let display = display.unwrap_or_else(drift_capture::sck::main_display);
    let keyboard = drift_macos::keyboard_type::detect();
    Box::new(InjectingInput::posting(display_map(&display), keyboard))
}
