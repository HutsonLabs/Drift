//! `RFX_AVC420_BITMAP_STREAM` parsing (MS-RDPEGFX 2.2.4.4), pure.
//!
//! ```text
//! RFX_AVC420_METABLOCK
//!   u32 numRegionRects
//!   RDPGFX_RECT16 regionRects[numRegionRects]        (left, top, right, bottom: u16 LE; right/bottom exclusive)
//!   RDPGFX_H264_QUANT_QUALITY quantQualityVals[...]  (qpVal: u8 = qp:6 | r:1 | p:1, qualityVal: u8)
//! avc420EncodedBitstream                             (the rest: an Annex-B access unit)
//! ```
//!
//! `drift-gfx` hands the bitmap data of a `WireToSurface1` PDU with codec AVC420 to
//! [`parse_avc420_bitmap_stream`]; the regions tell the compositor which rectangles of the decoded
//! picture changed (`FrameSink::blit_nv12`), and the bitstream goes to the decoder.

use drift_core::Rect;

/// Quantisation and quality of one region (`RDPGFX_H264_QUANT_QUALITY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantQuality {
    /// Quantisation parameter (0..=63).
    pub qp: u8,
    /// Progressive-encoding flag (`p` bit).
    pub progressive: bool,
    /// Quality level (0..=100).
    pub quality: u8,
}

/// A parsed `RFX_AVC420_METABLOCK`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Avc420Metablock {
    /// Changed regions of the picture (exclusive right/bottom converted to width/height).
    pub regions: Vec<Rect>,
    /// Per-region quantisation/quality values, same length as `regions`.
    pub quant_quality: Vec<QuantQuality>,
}

/// Errors from [`parse_avc420_bitmap_stream`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetablockError {
    /// The buffer ends inside the metablock.
    #[error("AVC420 metablock truncated: need {needed} bytes, have {available}")]
    Truncated {
        /// Bytes required by the declared region count.
        needed: usize,
        /// Bytes available.
        available: usize,
    },
    /// A rectangle has `right < left` or `bottom < top`.
    #[error("AVC420 region {index} is inverted")]
    InvertedRect {
        /// Index of the bad rectangle.
        index: usize,
    },
}

/// Parses an `RFX_AVC420_BITMAP_STREAM` into its metablock and the H.264 bitstream that follows.
pub fn parse_avc420_bitmap_stream(data: &[u8]) -> Result<(Avc420Metablock, &[u8]), MetablockError> {
    const RECT16: usize = 8;
    const QUANT_QUALITY: usize = 2;
    let truncated = |needed: usize| MetablockError::Truncated { needed, available: data.len() };

    let count_bytes: [u8; 4] = data.get(..4).and_then(|b| b.try_into().ok()).ok_or_else(|| truncated(4))?;
    let count = usize::try_from(u32::from_le_bytes(count_bytes)).unwrap_or(usize::MAX);
    let needed = count
        .checked_mul(RECT16 + QUANT_QUALITY)
        .and_then(|n| n.checked_add(4))
        .ok_or_else(|| truncated(usize::MAX))?;
    if data.len() < needed {
        return Err(truncated(needed));
    }

    let u16_at = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);
    let rects_start = 4;
    let quant_start = rects_start + count * RECT16;
    let mut meta =
        Avc420Metablock { regions: Vec::with_capacity(count), quant_quality: Vec::with_capacity(count) };
    for index in 0..count {
        let off = rects_start + index * RECT16;
        let [left, top, right, bottom] = [0, 2, 4, 6].map(|d| u32::from(u16_at(off + d)));
        let rect = Rect::from_ltrb(left, top, right, bottom).ok_or(MetablockError::InvertedRect { index })?;
        meta.regions.push(rect);
    }
    for index in 0..count {
        let off = quant_start + index * QUANT_QUALITY;
        let qp_val = data[off];
        meta.quant_quality.push(QuantQuality {
            qp: qp_val & 0x3F,
            progressive: qp_val & 0x80 != 0,
            quality: data[off + 1],
        });
    }
    Ok((meta, &data[needed..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(drift_testkit::fixtures::fixtures_dir().join(name)).unwrap()
    }

    /// The AVC420 stream of the MS-RDPEGFX AVC444 example (fixtures/pdus, see ADR).
    #[test]
    fn parses_msrdpegfx_example_bytes() {
        let data = fixture("pdus/avc420_bitmap_stream_msrdpegfx.bin");
        let (meta, h264) = parse_avc420_bitmap_stream(&data).unwrap();
        assert_eq!(meta.regions, vec![Rect::new(1792, 1056, 16, 16)]);
        assert_eq!(meta.quant_quality, vec![QuantQuality { qp: 22, progressive: false, quality: 100 }]);
        assert_eq!(h264.len(), 84 - 14);
        assert_eq!(&h264[..5], &[0, 0, 0, 1, 0x61]);
        let nals = crate::annexb::split_nals(h264);
        assert_eq!(nals.len(), 4);
    }

    /// A g-r-d shaped stream: one full-surface region wrapping the first real access unit.
    #[test]
    fn parses_full_surface_region_around_real_access_unit() {
        let leg3 = fixture("h264/leg3.h264");
        let au_end = leg3.windows(6).skip(4).position(|w| w == [0, 0, 0, 1, 9, 0x30]).unwrap() + 4;
        let au = &leg3[..au_end];
        let mut data = 1u32.to_le_bytes().to_vec();
        for v in [0u16, 0, 1280, 800] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend_from_slice(&[0x80 | 22, 100]);
        data.extend_from_slice(au);
        let (meta, h264) = parse_avc420_bitmap_stream(&data).unwrap();
        assert_eq!(meta.regions, vec![Rect::new(0, 0, 1280, 800)]);
        assert_eq!(meta.quant_quality, vec![QuantQuality { qp: 22, progressive: true, quality: 100 }]);
        assert_eq!(h264, au);
    }

    #[test]
    fn zero_regions_and_errors() {
        let (meta, rest) = parse_avc420_bitmap_stream(&[0, 0, 0, 0, 1, 2]).unwrap();
        assert!(meta.regions.is_empty());
        assert_eq!(rest, &[1, 2]);
        assert_eq!(
            parse_avc420_bitmap_stream(&[1, 0]),
            Err(MetablockError::Truncated { needed: 4, available: 2 })
        );
        assert_eq!(
            parse_avc420_bitmap_stream(&[2, 0, 0, 0, 0, 0]),
            Err(MetablockError::Truncated { needed: 24, available: 6 })
        );
        // a huge count must not allocate or overflow
        assert!(matches!(
            parse_avc420_bitmap_stream(&[0xFF, 0xFF, 0xFF, 0xFF]),
            Err(MetablockError::Truncated { .. })
        ));
        let mut inv = 1u32.to_le_bytes().to_vec();
        for v in [10u16, 0, 5, 8] {
            inv.extend_from_slice(&v.to_le_bytes());
        }
        inv.extend_from_slice(&[0, 0]);
        assert_eq!(parse_avc420_bitmap_stream(&inv), Err(MetablockError::InvertedRect { index: 0 }));
    }

    proptest! {
        /// Differential test against IronRDP's encoder for the same structure.
        #[test]
        fn matches_ironrdp_encoding(
            rects in proptest::collection::vec((0u16..4000, 0u16..4000, 0u16..200, 0u16..200, 0u8..64, any::<bool>(), 0u8..=100), 0..6),
            payload in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            use ironrdp_core::Encode as _;
            let stream = ironrdp_egfx::pdu::Avc420BitmapStream {
                rectangles: rects.iter().map(|&(l, t, w, h, ..)| ironrdp_pdu_rect(l, t, l.saturating_add(w), t.saturating_add(h))).collect(),
                quant_qual_vals: rects.iter().map(|&(.., qp, p, q)| ironrdp_egfx::pdu::QuantQuality { quantization_parameter: qp, progressive: p, quality: q }).collect(),
                data: &payload,
            };
            let mut buf = vec![0u8; stream.size()];
            stream.encode(&mut ironrdp_core::WriteCursor::new(&mut buf)).unwrap();
            let (meta, rest) = parse_avc420_bitmap_stream(&buf).unwrap();
            prop_assert_eq!(rest, &payload[..]);
            prop_assert_eq!(meta.regions.len(), rects.len());
            for (i, &(l, t, w, h, qp, p, q)) in rects.iter().enumerate() {
                let r = u32::from(l.saturating_add(w)) - u32::from(l);
                let b = u32::from(t.saturating_add(h)) - u32::from(t);
                prop_assert_eq!(meta.regions[i], Rect::new(l.into(), t.into(), r, b));
                prop_assert_eq!(meta.quant_quality[i], QuantQuality { qp, progressive: p, quality: q });
            }
        }

        #[test]
        fn never_panics(data in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = parse_avc420_bitmap_stream(&data);
        }
    }

    fn ironrdp_pdu_rect(
        left: u16,
        top: u16,
        right: u16,
        bottom: u16,
    ) -> ironrdp_pdu::geometry::ExclusiveRectangle {
        ironrdp_pdu::geometry::ExclusiveRectangle { left, top, right, bottom }
    }
}
