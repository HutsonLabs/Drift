//! HOST-11 Red: the host's rdpsnd server streams PCM to Drift's client, whose audio output gets
//! the samples. The source is a synthetic tone (ScreenCaptureKit audio needs Screen Recording).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "support/host.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_core::audio::PcmFormat;
use drift_host::audio::{AudioSource, ToneSource};
use drift_rdp::audio::{AudioOutput, AudioSink};
use drift_rdp::{SessionCommand, SessionOptions};
use support::*;

#[derive(Default)]
struct Recorded {
    format: Option<PcmFormat>,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct RecordingSink(Arc<Mutex<Recorded>>);

impl AudioSink for RecordingSink {
    fn start(&self, format: PcmFormat) {
        self.0.lock().unwrap().format = Some(format);
    }
    fn play(&self, pcm: &[u8]) {
        self.0.lock().unwrap().bytes.extend_from_slice(pcm);
    }
    fn stop(&self) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn host_tone_reaches_the_client_audio_output() {
    let dir = tempfile::tempdir().unwrap();
    let (host, identity) =
        start_host(dir.path(), None, Some(Box::new(|| Box::new(ToneSource::new()) as Box<dyn AudioSource>)));
    let sink = RecordingSink::default();
    let (handle, mut events) = connect(
        &host,
        &identity,
        SessionOptions { audio: Some(AudioOutput::new(sink.clone())), ..SessionOptions::default() },
    );
    wait_event(&mut events, "connected", connected).await;

    // 0.3 s of 48 kHz stereo 16-bit PCM.
    wait_until("audio at the client", Duration::from_secs(15), || {
        sink.0.lock().unwrap().bytes.len() >= 48_000 * 4 * 3 / 10
    })
    .await;
    let rec = sink.0.lock().unwrap();
    assert_eq!(rec.format, Some(PcmFormat::STEREO_48K_S16));
    // Every stereo frame of the tone satisfies R == L ^ 0x5555, so the bytes arrived intact and
    // in frame order even if a wave was skipped.
    for frame in rec.bytes.chunks_exact(4) {
        let l = i16::from_le_bytes([frame[0], frame[1]]);
        let r = i16::from_le_bytes([frame[2], frame[3]]);
        assert_eq!(r, l ^ 0x5555, "frame {frame:?}");
    }
    assert!(host.stats().audio_waves > 0, "{:?}", host.stats());
    drop(rec);
    let _ = handle.send(SessionCommand::Close);
}

#[test]
fn tone_frames_keep_their_invariant() {
    let mut tone = ToneSource::new();
    let pcm = tone.render(480);
    assert_eq!(pcm.len(), 480 * 4);
    for frame in pcm.chunks_exact(4) {
        let l = i16::from_le_bytes([frame[0], frame[1]]);
        let r = i16::from_le_bytes([frame[2], frame[3]]);
        assert_eq!(r, l ^ 0x5555);
    }
}
