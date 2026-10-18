//! EXT-5 Red, client half (ADR `EXT-5-hevc-hdr`): `WireToSurface1` with Drift's private HEVC codec
//! id (`Codec1Type::DriftHevc`, sent only to a client that negotiated it on `Drift::Mac::v1`)
//! carries an AVC420-style metablock and one HEVC access unit; the client decodes it with the
//! session's HEVC decoder and blits the metablock regions — NV12 for 4:2:0, BGRA for 4:4:4.
//! Without a HEVC decoder (every non-Drift host) the codec stays unsupported.
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::{Arc, Mutex};

use drift_core::video::{DecodeError, DecodedVideo, HevcDecoder, Transfer, VideoPicture};
use drift_core::{BgraPicture, Nv12Frame, Nv12Planes, Rect, Size};
use drift_gfx::GfxError;
use drift_testkit::{FrameSinkCall, PresentMode};
use ironrdp_core::encode_vec;
use ironrdp_egfx::pdu::{Avc420BitmapStream, Codec1Type, QuantQuality};
use support::{create, end, harness, map, rect16, reset, start, w2s1, wire};

#[derive(Clone)]
struct FakeHevc {
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    resets: Arc<Mutex<u32>>,
    full_chroma: bool,
}

impl FakeHevc {
    fn new(full_chroma: bool) -> Self {
        Self { seen: Arc::default(), resets: Arc::default(), full_chroma }
    }
}

impl HevcDecoder for FakeHevc {
    fn decode(&mut self, annex_b: &[u8]) -> Result<Option<DecodedVideo>, DecodeError> {
        self.seen.lock().unwrap().push(annex_b.to_vec());
        let size = Size::new(64, 32);
        let picture = if self.full_chroma {
            VideoPicture::Bgra(BgraPicture::new(size, vec![9; 64 * 32 * 4]).unwrap())
        } else {
            VideoPicture::Nv12(Nv12Frame::new(
                Nv12Planes::new(size, vec![1; 64 * 32], vec![2; 64 * 16]).unwrap(),
            ))
        };
        Ok(Some(DecodedVideo { picture, transfer: Transfer::Sdr }))
    }

    fn reset(&mut self) {
        *self.resets.lock().unwrap() += 1;
    }
}

fn stream(rect: (u16, u16, u16, u16), data: &'static [u8]) -> Vec<u8> {
    encode_vec(&Avc420BitmapStream {
        rectangles: vec![rect16(rect.0, rect.1, rect.2, rect.3)],
        quant_qual_vals: vec![QuantQuality { quantization_parameter: 22, progressive: false, quality: 100 }],
        data,
    })
    .unwrap()
}

fn setup(decoder: Option<FakeHevc>) -> support::Harness {
    let mut h = harness(PresentMode::Immediate, Size::new(64, 32));
    if let Some(d) = decoder {
        h.client.set_hevc(Box::new(d));
    }
    h.client.process_payload(&wire(&[reset(64, 32), create(1, 64, 32), map(1, 0, 0)])).unwrap();
    h
}

#[test]
fn hevc_420_frames_blit_nv12_regions() {
    let dec = FakeHevc::new(false);
    let mut h = setup(Some(dec.clone()));
    let data = stream((0, 0, 16, 8), b"\0\0\0\x01\x40\x01hevc");
    h.client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::DriftHevc, rect16(0, 0, 64, 32), data),
            end(1),
        ]))
        .unwrap();
    assert_eq!(dec.seen.lock().unwrap().clone(), vec![b"\0\0\0\x01\x40\x01hevc".to_vec()]);
    let blits: Vec<Vec<Rect>> = h
        .log
        .calls()
        .iter()
        .filter_map(|c| match c {
            FrameSinkCall::BlitNv12 { id: 1, regions, .. } => Some(regions.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(blits, vec![vec![Rect::new(0, 0, 16, 8)]]);
}

#[test]
fn hevc_444_frames_blit_bgra_regions() {
    let mut h = setup(Some(FakeHevc::new(true)));
    let data = stream((8, 8, 24, 16), b"\0\0\0\x01\x26\x01x");
    h.client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::DriftHevc, rect16(0, 0, 64, 32), data),
            end(1),
        ]))
        .unwrap();
    let blits: Vec<Rect> = h
        .log
        .calls()
        .iter()
        .filter_map(|c| match c {
            FrameSinkCall::BlitBgra { id: 1, rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(blits, vec![Rect::new(8, 8, 16, 8)]);
}

#[test]
fn without_a_hevc_decoder_the_private_codec_is_unsupported() {
    let mut h = setup(None);
    let data = stream((0, 0, 64, 32), b"\0\0\0\x01\x40\x01x");
    let err = h
        .client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::DriftHevc, rect16(0, 0, 64, 32), data),
            end(1),
        ]))
        .unwrap_err();
    assert!(matches!(err, GfxError::UnsupportedCodec(_)), "{err:?}");
}

#[test]
fn reset_graphics_resets_the_hevc_decoder() {
    let dec = FakeHevc::new(false);
    let mut h = setup(Some(dec.clone()));
    let before = *dec.resets.lock().unwrap();
    h.client.process_payload(&wire(&[reset(64, 32)])).unwrap();
    assert_eq!(*dec.resets.lock().unwrap(), before + 1);
}

#[test]
fn the_private_codec_id_is_outside_ms_rdpegfx() {
    // MS-RDPEGFX 2.2.2.1 assigns 0x0000–0x000F; Drift's id must never collide with a real codec.
    assert!(u16::from(Codec1Type::DriftHevc) > 0x00FF);
    assert_eq!(Codec1Type::try_from(u16::from(Codec1Type::DriftHevc)).unwrap(), Codec1Type::DriftHevc);
}
