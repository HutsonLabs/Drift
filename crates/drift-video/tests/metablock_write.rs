//! HOST-4 Red: the host writes what the client parses — `RFX_AVC420_BITMAP_STREAM` and
//! `RFX_AVC444_BITMAP_STREAM` (MS-RDPEGFX 2.2.4.4, 2.2.4.5).
#![allow(clippy::unwrap_used)]

use drift_core::Rect;
use drift_video::metablock::{
    Avc420Metablock, Avc444Layout, QuantQuality, parse_avc420_bitmap_stream, parse_avc444_bitmap_stream,
    write_avc420_bitmap_stream, write_avc444_bitmap_stream,
};

fn meta() -> Avc420Metablock {
    Avc420Metablock {
        regions: vec![Rect::new(0, 0, 64, 32), Rect::new(100, 40, 8, 8)],
        quant_quality: vec![
            QuantQuality { qp: 22, progressive: false, quality: 100 },
            QuantQuality { qp: 30, progressive: true, quality: 70 },
        ],
    }
}

#[test]
fn avc420_round_trips_through_the_client_parser() {
    let bitstream = [0u8, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x65, 0x88];
    let bytes = write_avc420_bitmap_stream(&meta(), &bitstream);
    let (parsed, rest) = parse_avc420_bitmap_stream(&bytes).unwrap();
    assert_eq!(parsed, meta());
    assert_eq!(rest, bitstream);
    assert_eq!(&bytes[..4], &2u32.to_le_bytes(), "numRegionRects");
}

#[test]
fn avc444_header_packs_size_and_layout() {
    let one = write_avc420_bitmap_stream(&meta(), &[1, 2, 3]);
    let two = write_avc420_bitmap_stream(&meta(), &[4, 5]);
    let both = write_avc444_bitmap_stream(Avc444Layout::LumaAndChroma, &one, Some(&two));
    let info = u32::from_le_bytes(both[..4].try_into().unwrap());
    assert_eq!(info & 0x3FFF_FFFF, u32::try_from(one.len()).unwrap());
    assert_eq!(info >> 30, 0, "LC = 0: both views");
    let (layout, s1, s2) = parse_avc444_bitmap_stream(&both).unwrap();
    assert_eq!((layout, s1, s2), (Avc444Layout::LumaAndChroma, &one[..], Some(&two[..])));

    let luma = write_avc444_bitmap_stream(Avc444Layout::LumaOnly, &one, None);
    assert_eq!(u32::from_le_bytes(luma[..4].try_into().unwrap()) >> 30, 1);
    assert_eq!(parse_avc444_bitmap_stream(&luma).unwrap(), (Avc444Layout::LumaOnly, &one[..], None));

    let chroma = write_avc444_bitmap_stream(Avc444Layout::ChromaOnly, &two, None);
    assert_eq!(u32::from_le_bytes(chroma[..4].try_into().unwrap()) >> 30, 2);
}

#[test]
fn malformed_avc444_streams_are_errors() {
    assert!(parse_avc444_bitmap_stream(&[1, 0]).is_err());
    // claims 100 bytes of stream 1 but has 2
    let mut bad = 100u32.to_le_bytes().to_vec();
    bad.extend_from_slice(&[0, 0]);
    assert!(parse_avc444_bitmap_stream(&bad).is_err());
    // LC = 3 is reserved
    let mut reserved = (3u32 << 30).to_le_bytes().to_vec();
    reserved.extend_from_slice(&[0; 4]);
    assert!(parse_avc444_bitmap_stream(&reserved).is_err());
}
