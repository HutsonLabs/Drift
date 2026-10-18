//! AVC444 end to end, client half (ADR `AVC444-client`): `WireToSurface1` with codec AVC444
//! goes to the session's AVC444 decoder, which turns the main (luma) and auxiliary (chroma)
//! views into one full-chroma BGRA picture; the client blits the metablock regions of it.
//! Without an AVC444 decoder (GNOME sessions) the codec stays unsupported.
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::{Arc, Mutex};

use drift_core::video::{Avc444Decoder, BgraPicture, DecodeError};
use drift_core::{Rect, Size};
use drift_gfx::GfxError;
use drift_testkit::{FrameSinkCall, PresentMode};
use ironrdp_core::encode_vec;
use ironrdp_egfx::pdu::{Avc420BitmapStream, Avc444BitmapStream, Codec1Type, Encoding, QuantQuality};
use support::{create, end, harness, map, rect16, reset, start, w2s1, wire};

/// Records what it was asked to decode (rectangles are exclusive, as on the wire) and returns a 64×32 picture whose pixel (x, y) is
/// `[x, y, 7, 255]`.
#[derive(Clone, Default)]
struct FakeAvc444 {
    seen: Arc<Mutex<Vec<(Option<Vec<u8>>, Option<Vec<u8>>)>>>,
    resets: Arc<Mutex<u32>>,
}

impl Avc444Decoder for FakeAvc444 {
    fn decode(
        &mut self,
        main: Option<&[u8]>,
        aux: Option<&[u8]>,
    ) -> Result<Option<BgraPicture>, DecodeError> {
        self.seen.lock().unwrap().push((main.map(<[u8]>::to_vec), aux.map(<[u8]>::to_vec)));
        let size = Size::new(64, 32);
        let mut data = Vec::with_capacity(64 * 32 * 4);
        for y in 0..32u8 {
            for x in 0..64u8 {
                data.extend_from_slice(&[x, y, 7, 255]);
            }
        }
        Ok(BgraPicture::new(size, data))
    }

    fn reset(&mut self) {
        *self.resets.lock().unwrap() += 1;
    }
}

fn avc420(rect: (u16, u16, u16, u16), data: &'static [u8]) -> Avc420BitmapStream<'static> {
    Avc420BitmapStream {
        rectangles: vec![rect16(rect.0, rect.1, rect.2, rect.3)],
        quant_qual_vals: vec![QuantQuality { quantization_parameter: 22, progressive: false, quality: 100 }],
        data,
    }
}

fn avc444(
    encoding: Encoding,
    s1: Avc420BitmapStream<'static>,
    s2: Option<Avc420BitmapStream<'static>>,
) -> Vec<u8> {
    encode_vec(&Avc444BitmapStream { encoding, stream1: s1, stream2: s2 }).unwrap()
}

fn setup() -> (support::Harness, FakeAvc444) {
    let mut h = harness(PresentMode::Immediate, Size::new(64, 32));
    let dec = FakeAvc444::default();
    h.client.set_avc444(Box::new(dec.clone()));
    h.client.process_payload(&wire(&[reset(64, 32), create(1, 64, 32), map(1, 0, 0)])).unwrap();
    (h, dec)
}

fn blits(calls: &[FrameSinkCall]) -> Vec<(Rect, usize)> {
    calls
        .iter()
        .filter_map(|c| match c {
            FrameSinkCall::BlitBgra { id: 1, rect, stride, .. } => Some((*rect, *stride)),
            _ => None,
        })
        .collect()
}

#[test]
fn luma_and_chroma_decode_both_views_and_blit_the_regions() {
    let (h, dec) = setup();
    let mut h = h;
    let data = avc444(
        Encoding::LUMA_AND_CHROMA,
        avc420((0, 0, 16, 8), b"\0\0\0\x01main"),
        Some(avc420((0, 0, 64, 32), b"\0\0\0\x01aux")),
    );
    h.client
        .process_payload(&wire(&[start(1), w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), data), end(1)]))
        .unwrap();
    let seen = dec.seen.lock().unwrap().clone();
    assert_eq!(seen, vec![(Some(b"\0\0\0\x01main".to_vec()), Some(b"\0\0\0\x01aux".to_vec()))]);
    // The luma view's regions say what changed.
    assert_eq!(blits(&h.log.calls()), vec![(Rect::new(0, 0, 16, 8), 64 * 4)]);
}

#[test]
fn luma_only_and_chroma_only_pass_one_view() {
    let (mut h, dec) = setup();
    let luma = avc444(Encoding::LUMA, avc420((0, 0, 64, 32), b"\0\0\0\x01luma"), None);
    let chroma = avc444(Encoding::CHROMA, avc420((8, 8, 24, 16), b"\0\0\0\x01chroma"), None);
    h.client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), luma),
            end(1),
            start(2),
            w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), chroma),
            end(2),
        ]))
        .unwrap();
    let seen = dec.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![(Some(b"\0\0\0\x01luma".to_vec()), None), (None, Some(b"\0\0\0\x01chroma".to_vec()))]
    );
    assert_eq!(
        blits(&h.log.calls()),
        vec![(Rect::new(0, 0, 64, 32), 64 * 4), (Rect::new(8, 8, 16, 8), 64 * 4)]
    );
}

#[test]
fn regions_outside_the_surface_are_clipped() {
    let (mut h, _dec) = setup();
    let data = avc444(Encoding::LUMA, avc420((48, 16, 100, 100), b"\0\0\0\x01x"), None);
    h.client
        .process_payload(&wire(&[start(1), w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), data), end(1)]))
        .unwrap();
    assert_eq!(blits(&h.log.calls()), vec![(Rect::new(48, 16, 16, 16), 64 * 4)]);
}

#[test]
fn reset_graphics_resets_the_avc444_decoder() {
    let (mut h, dec) = setup();
    h.client.process_payload(&wire(&[reset(64, 32)])).unwrap();
    assert!(*dec.resets.lock().unwrap() >= 1);
}

#[test]
fn without_an_avc444_decoder_the_codec_is_unsupported() {
    let mut h = harness(PresentMode::Immediate, Size::new(64, 32));
    h.client.process_payload(&wire(&[reset(64, 32), create(1, 64, 32), map(1, 0, 0)])).unwrap();
    let data = avc444(Encoding::LUMA, avc420((0, 0, 64, 32), b"\0\0\0\x01x"), None);
    let err = h
        .client
        .process_payload(&wire(&[start(1), w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), data), end(1)]))
        .unwrap_err();
    assert!(matches!(err, GfxError::UnsupportedCodec(_)), "{err:?}");
}

#[test]
fn avc444v2_stays_unsupported_even_with_a_decoder() {
    let (mut h, _dec) = setup();
    let data = avc444(Encoding::LUMA, avc420((0, 0, 64, 32), b"\0\0\0\x01x"), None);
    let err = h
        .client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::Avc444v2, rect16(0, 0, 64, 32), data),
            end(1),
        ]))
        .unwrap_err();
    assert!(matches!(err, GfxError::UnsupportedCodec(_)), "{err:?}");
}

#[test]
fn a_malformed_avc444_stream_is_a_decode_error() {
    let (mut h, _dec) = setup();
    let err = h
        .client
        .process_payload(&wire(&[
            start(1),
            w2s1(1, Codec1Type::Avc444, rect16(0, 0, 64, 32), vec![0xFF, 0xFF, 0xFF, 0xFF, 1]),
            end(1),
        ]))
        .unwrap_err();
    assert!(matches!(err, GfxError::Decode(_)), "{err:?}");
}
