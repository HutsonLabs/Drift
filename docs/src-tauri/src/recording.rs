//! Session recording hook (task **M8-3**, app part; cargo feature `recording`).
//!
//! The hidden menu item "Debug ▸ Record Session (experimental)" toggles this. Drift takes the
//! *composite* frames the session window's compositor already captures (M8-1), encodes them with the
//! VideoToolbox encoder (M8-2) and appends them to an MP4 in passthrough mode (M8-3 writer).
//! Everything runs on one dedicated thread per recording; the render thread only hands over
//! IOSurface-backed pixel buffers, so nothing is copied on the session's hot path.

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use drift_render::{CaptureConfig, Compositor, LayerTarget, RenderThread};
use drift_video::encode::{EncoderConfig, PixelBuffer, VtEncoder};
use drift_video::mp4::{AppendOutcome, Recorder, RecordingEvent};
use objc2_core_foundation::CFRetained;

/// A running recording; dropping or [`Recording::stop`]ping it finalises the file.
pub struct Recording {
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording").field("path", &self.path).finish()
    }
}

impl Recording {
    /// Waits for the encoder thread to finalise the file.
    pub fn stop(mut self) {
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the recording thread panicked");
        }
        tracing::info!(path = %self.path.display(), "recording stopped");
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Where a recording is written: `~/Movies/Drift <profile> <timestamp>.mp4`.
fn output_path(profile_name: &str) -> PathBuf {
    let safe: String = profile_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let dir = std::env::var_os("HOME").map_or_else(
        || PathBuf::from("/tmp"),
        |home| {
            let movies = PathBuf::from(&home).join("Movies");
            if movies.is_dir() { movies } else { PathBuf::from(home) }
        },
    );
    dir.join(format!("Drift {safe} {stamp}.mp4"))
}

/// Starts capturing, encoding and writing the key session window's composite.
///
/// Stop it by calling `Compositor::stop_recording` on the same render thread (which ends the
/// frame stream) and then [`Recording::stop`].
pub fn start(
    render: &RenderThread<Compositor<LayerTarget>>,
    profile_name: &str,
) -> Result<Recording, String> {
    let path = output_path(profile_name);
    let frames = render
        .with(|compositor| compositor.start_recording(CaptureConfig::default()))
        .ok_or_else(|| "the render thread has stopped".to_owned())?;
    let (events_tx, events_rx) = channel::<RecordingEvent>();
    std::thread::spawn(move || {
        for event in events_rx {
            tracing::info!(?event, "recording");
        }
    });
    let output = path.clone();
    let thread = std::thread::Builder::new()
        .name("drift-recording".to_owned())
        .spawn(move || {
            let mut recorder = Recorder::new(events_tx);
            if let Err(e) = recorder.start(&output) {
                tracing::error!(error = %e, "could not start the MP4 writer");
                return;
            }
            let mut encoder: Option<VtEncoder> = None;
            for frame in frames {
                let size = frame.size();
                if encoder.as_ref().is_none_or(|e| e.config().size != size) {
                    match VtEncoder::new(EncoderConfig::new(size)) {
                        Ok(new) => encoder = Some(new),
                        Err(e) => {
                            tracing::error!(error = %e, "could not build the encoder");
                            break;
                        }
                    }
                }
                let Some(encoder) = encoder.as_mut() else { break };
                // SAFETY: the frame owns a live `CVPixelBuffer`; `retain` takes the extra
                // reference the `CFRetained` handle releases when the encoder is done.
                let buffer = unsafe { CFRetained::retain(std::ptr::NonNull::from(frame.pixel_buffer())) };
                let encoded = match encoder.encode(&PixelBuffer::from_cv(buffer), Instant::now()) {
                    Ok(encoded) => encoded,
                    Err(e) => {
                        tracing::error!(error = %e, "encode failed");
                        break;
                    }
                };
                if append(&mut recorder, &encoded) {
                    return;
                }
            }
            if let Some(mut encoder) = encoder
                && let Ok(tail) = encoder.flush()
                && append(&mut recorder, &tail)
            {
                return;
            }
            if let Err(e) = recorder.stop() {
                tracing::error!(error = %e, "could not finalise the recording");
            }
        })
        .map_err(|e| e.to_string())?;
    tracing::info!(path = %path.display(), "recording started");
    Ok(Recording { path, thread: Some(thread) })
}

/// Appends frames; `true` when the recorder stopped by itself (disk full, writer error).
fn append(recorder: &mut Recorder, frames: &[drift_video::encode::EncodedFrame]) -> bool {
    for frame in frames {
        match recorder.append(frame) {
            Ok(AppendOutcome::Recording) => {}
            Ok(AppendOutcome::Stopped(reason)) => {
                tracing::warn!(?reason, "recording stopped early");
                return true;
            }
            Err(e) => {
                tracing::error!(error = %e, "append failed");
                return true;
            }
        }
    }
    false
}
