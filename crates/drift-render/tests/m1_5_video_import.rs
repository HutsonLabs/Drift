//! M1-5: frames decoded by drift-video (`DecodedPicture`, IOSurface NV12) are imported
//! zero-copy by the compositor without extra wiring.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use drift_core::{Nv12Frame, Rect, Size};
use drift_gfx::FrameSink;
use drift_video::annexb::{nal_type, nal_unit_type, split_nals};
use drift_video::decode::VtDecoder;

fn first_picture() -> drift_video::DecodedPicture {
    let stream = std::fs::read(drift_testkit::fixtures::fixtures_dir().join("h264/leg3.h264")).unwrap();
    let mut dec = VtDecoder::new();
    let mut au = Vec::new();
    let mut flush = |au: &mut Vec<u8>| {
        let picture = if au.is_empty() { None } else { dec.decode_picture(au).unwrap() };
        au.clear();
        picture
    };
    for nal in split_nals(&stream) {
        if nal_unit_type(nal) == Some(nal_type::AUD)
            && let Some(p) = flush(&mut au)
        {
            return p;
        }
        au.extend_from_slice(&[0, 0, 0, 1]);
        au.extend_from_slice(nal);
    }
    flush(&mut au).expect("leg3 decodes at least one picture")
}

#[test]
fn decoded_pictures_render_like_their_cpu_planes() {
    let picture = first_picture();
    assert!(picture.is_iosurface_backed());
    let planes = picture.to_planes().unwrap();
    let size = Nv12Frame::new(planes.clone()).size();
    let full = [Rect::new(0, 0, size.width, size.height)];
    let render = |frame: Nv12Frame| {
        let mut gpu = offscreen(Size::new(8, 8));
        gpu.reset(size);
        gpu.create_surface(1, size);
        gpu.blit_nv12(1, &frame, &full);
        gpu.wait_idle();
        gpu.read_surface(1).unwrap()
    };
    let zero_copy = render(Nv12Frame::new(picture));
    let uploaded = render(Nv12Frame::new(planes));
    assert_eq!(zero_copy, uploaded);
    assert!(zero_copy.data.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]), "picture was not drawn");
}
