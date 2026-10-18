//! Server-side virtual channels of the [`crate::FakeServer`] (M4-2, M5-2, M6-3 loopback tests).
//!
//! g-r-d offers three channels Drift uses after activation (plan §1.4, §1.6):
//!
//! - **DRDYNVC → `Microsoft::Windows::DisplayControl`** ([`FakeDisplayControl`]): sends the
//!   capabilities and records every monitor layout the client requests. Like g-r-d, each
//!   layout makes the graphics pipeline send `ResetGraphics` plus a **new surface id**.
//! - **DRDYNVC → `Microsoft::Windows::RDS::Graphics`** ([`FakeGraphics`]): answers the client's
//!   `CapabilitiesAdvertise` with `CapabilitiesConfirm(V8.1)`, then draws one frame (reset,
//!   surface, map, solid fill) and records `FrameAcknowledge`s, including suspends.
//! - **CLIPRDR** (`CliprdrServer` + [`FakeClipboardBackend`]): records the client's format
//!   lists, data requests and data responses, and serves scripted clipboard contents.
//!
//! Everything is recorded into the leg's [`crate::LegRecord`].

use std::sync::{Arc, Mutex, PoisonError};

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardGeneralCapabilityFlags, FileContentsRequest, FileContentsResponse,
    FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_core::{EncodeResult, WriteCursor, impl_as_any};
use ironrdp_displaycontrol::pdu::{DisplayControlCapabilities, DisplayControlPdu};
use ironrdp_dvc::{DvcEncode, DvcMessage, DvcProcessor, DvcServerProcessor};
use ironrdp_egfx::pdu::{
    CapabilitiesConfirmPdu, CapabilitiesV81Flags, CapabilitySet, Color, CreateSurfacePdu, EndFramePdu,
    GfxPdu, MapSurfaceToOutputPdu, PixelFormat, QueueDepth, ResetGraphicsPdu, SolidFillPdu, StartFramePdu,
    Timestamp,
};
use ironrdp_pdu::geometry::ExclusiveRectangle;
use ironrdp_pdu::{PduResult, decode_err, pdu_other_err};

/// Which virtual channels a fake server leg offers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Channels {
    /// Display Control (absent in Desktop Sharing, plan §1.2).
    pub display_control: bool,
    /// Graphics pipeline.
    pub gfx: bool,
    /// Clipboard.
    pub clipboard: bool,
    /// The `Drift::Mac::v1` extension channel, as a Drift Host opens it (EXT-1).
    pub drift_mac: bool,
}

impl Channels {
    /// Every channel (Remote Login / Headless).
    pub const fn all() -> Self {
        Self { display_control: true, gfx: true, clipboard: true, drift_mac: false }
    }

    /// What g-r-d's Desktop Sharing daemon offers: no Display Control.
    pub const fn desktop_sharing() -> Self {
        Self { display_control: false, gfx: true, clipboard: true, drift_mac: false }
    }

    /// The same channels plus `Drift::Mac::v1`, opened by the server like a Drift Host does.
    #[must_use]
    pub const fn with_drift_mac(mut self) -> Self {
        self.drift_mac = true;
        self
    }

    /// `true` when DRDYNVC must be attached.
    pub const fn any_dynamic(&self) -> bool {
        self.display_control || self.gfx || self.drift_mac
    }
}

/// One clipboard format served by the fake server (a scripted remote copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerClipFormat {
    /// Format id.
    pub id: u32,
    /// Registered format name (e.g. `"image/png"`).
    pub name: Option<String>,
    /// The bytes returned for a Format Data Request of this format.
    pub data: Vec<u8>,
}

impl ServerClipFormat {
    /// `CF_UNICODETEXT` with `text` (UTF-16LE plus NUL, as g-r-d sends it).
    pub fn unicode_text(text: &str) -> Self {
        let mut data: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        data.extend_from_slice(&[0, 0]);
        Self { id: 13, name: None, data }
    }

    /// The named `"image/png"` format with id `0xD011` (as captured from g-r-d, plan §1.6).
    pub fn png(png: Vec<u8>) -> Self {
        Self { id: 0xD011, name: Some("image/png".into()), data: png }
    }
}

/// A monitor layout the client requested over Display Control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedLayout {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `DesktopScaleFactor` in percent.
    pub desktop_scale: u32,
}

/// Shared state between the channel processors and the server loop of one leg.
#[derive(Debug, Default)]
pub(crate) struct ChannelState {
    /// Layouts received; the loop drains `pending_resets` to redraw.
    pub(crate) layouts: Vec<RecordedLayout>,
    pub(crate) pending_resets: Vec<(u32, u32)>,
    pub(crate) gfx_caps_advertised: bool,
    pub(crate) gfx_frame_acks: Vec<u32>,
    pub(crate) gfx_suspend_acks: u32,
    pub(crate) gfx_resets_sent: Vec<(u32, u32)>,
    pub(crate) gfx_fills_sent: u32,
    /// The server end of `Drift::Mac::v1`, when offered.
    pub(crate) drift_mac: Option<drift_ext::channel::Link>,
    /// Clipboard backend events not yet handled by the loop.
    pub(crate) clip_events: Vec<ClipEvent>,
}

pub(crate) type SharedState = Arc<Mutex<ChannelState>>;

pub(crate) fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, ChannelState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A clipboard backend callback, handled by the server loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipEvent {
    Ready,
    ClientFormatList(Vec<(u32, Option<String>)>),
    DataRequest(u32),
    DataResponse(Option<Vec<u8>>),
}

/// Raw bytes as a DVC message (already ZGFX-wrapped GFX data).
struct RawDvc(Vec<u8>);

impl ironrdp_core::Encode for RawDvc {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ironrdp_core::ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "RawDvc"
    }

    fn size(&self) -> usize {
        self.0.len()
    }
}

impl DvcEncode for RawDvc {}

/// Server end of Display Control.
pub(crate) struct FakeDisplayControl {
    pub(crate) state: SharedState,
}

impl_as_any!(FakeDisplayControl);

impl DvcProcessor for FakeDisplayControl {
    fn channel_name(&self) -> &str {
        ironrdp_displaycontrol::CHANNEL_NAME
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        let caps = DisplayControlCapabilities::new(1, 8192, 8192).map_err(|e| decode_err!(e))?;
        Ok(vec![Box::new(DisplayControlPdu::from(caps))])
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        if let DisplayControlPdu::MonitorLayout(layout) =
            ironrdp_core::decode::<DisplayControlPdu>(payload).map_err(|e| decode_err!(e))?
        {
            let Some(primary) = layout.monitors().first() else {
                return Err(pdu_other_err!("fake DISP", "empty monitor layout"));
            };
            let (width, height) = primary.dimensions();
            let desktop_scale = primary.desktop_scale_factor().unwrap_or(100);
            let mut s = lock(&self.state);
            s.layouts.push(RecordedLayout { width, height, desktop_scale });
            s.pending_resets.push((width, height));
        }
        Ok(Vec::new())
    }
}

impl DvcServerProcessor for FakeDisplayControl {}

/// Server end of the graphics pipeline.
pub(crate) struct FakeGraphics {
    pub(crate) state: SharedState,
    pub(crate) desktop: (u32, u32),
}

impl_as_any!(FakeGraphics);

impl DvcProcessor for FakeGraphics {
    fn channel_name(&self) -> &str {
        "Microsoft::Windows::RDS::Graphics"
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(Vec::new())
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        match ironrdp_core::decode::<GfxPdu>(payload).map_err(|e| decode_err!(e))? {
            GfxPdu::CapabilitiesAdvertise(_) => {
                lock(&self.state).gfx_caps_advertised = true;
                let confirm =
                    GfxPdu::CapabilitiesConfirm(CapabilitiesConfirmPdu::from_typed(&CapabilitySet::V8_1 {
                        flags: CapabilitiesV81Flags::AVC420_ENABLED,
                    }));
                let (w, h) = self.desktop;
                let mut pdus = vec![confirm];
                pdus.extend(redraw_pdus(&self.state, w, h));
                Ok(vec![gfx_message(&pdus)?])
            }
            GfxPdu::FrameAcknowledge(ack) => {
                let mut s = lock(&self.state);
                if ack.queue_depth == QueueDepth::Suspend {
                    s.gfx_suspend_acks += 1;
                }
                s.gfx_frame_acks.push(ack.frame_id);
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }
}

impl DvcServerProcessor for FakeGraphics {}

/// `ResetGraphics(w×h)` + a new surface covering it + one solid-filled frame.
pub(crate) fn redraw_pdus(state: &SharedState, width: u32, height: u32) -> Vec<GfxPdu> {
    let (surface_id, frame_id) = {
        let mut s = lock(state);
        s.gfx_resets_sent.push((width, height));
        let n = u16::try_from(s.gfx_resets_sent.len()).unwrap_or(u16::MAX);
        (n, u32::from(n))
    };
    let w16 = u16::try_from(width).unwrap_or(u16::MAX);
    let h16 = u16::try_from(height).unwrap_or(u16::MAX);
    vec![
        GfxPdu::ResetGraphics(ResetGraphicsPdu { width, height, monitors: Vec::new() }),
        GfxPdu::CreateSurface(CreateSurfacePdu {
            surface_id,
            width: w16,
            height: h16,
            pixel_format: PixelFormat::XRgb,
        }),
        GfxPdu::MapSurfaceToOutput(MapSurfaceToOutputPdu {
            surface_id,
            output_origin_x: 0,
            output_origin_y: 0,
        }),
        GfxPdu::StartFrame(StartFramePdu {
            timestamp: Timestamp { milliseconds: 0, seconds: 0, minutes: 0, hours: 0 },
            frame_id,
        }),
        GfxPdu::SolidFill(SolidFillPdu {
            surface_id,
            fill_pixel: Color { b: 0x20, g: 0x40, r: 0x60, xa: 0xFF },
            rectangles: vec![ExclusiveRectangle { left: 0, top: 0, right: w16, bottom: h16 }],
        }),
        GfxPdu::EndFrame(EndFramePdu { frame_id }),
    ]
}

/// One frame filling `rect` of the newest surface (see [`redraw_pdus`]) with `color`.
pub(crate) fn fill_pdus(state: &SharedState, rect: (u16, u16, u16, u16), color: [u8; 3]) -> Vec<GfxPdu> {
    let (surface_id, frame_id) = {
        let mut s = lock(state);
        s.gfx_fills_sent += 1;
        let surface = u16::try_from(s.gfx_resets_sent.len()).unwrap_or(u16::MAX);
        (surface, 0x1_0000 + s.gfx_fills_sent)
    };
    let (left, top, right, bottom) = rect;
    let [r, g, b] = color;
    vec![
        GfxPdu::StartFrame(StartFramePdu {
            timestamp: Timestamp { milliseconds: 0, seconds: 0, minutes: 0, hours: 0 },
            frame_id,
        }),
        GfxPdu::SolidFill(SolidFillPdu {
            surface_id,
            fill_pixel: Color { b, g, r, xa: 0xFF },
            rectangles: vec![ExclusiveRectangle { left, top, right, bottom }],
        }),
        GfxPdu::EndFrame(EndFramePdu { frame_id }),
    ]
}

/// Encodes GFX PDUs as one uncompressed RDP_SEGMENTED_DATA message (what g-r-d sends, minus
/// the compression).
pub(crate) fn gfx_message(pdus: &[GfxPdu]) -> PduResult<DvcMessage> {
    let mut bytes = Vec::new();
    for pdu in pdus {
        bytes.extend(ironrdp_core::encode_vec(pdu).map_err(|e| pdu_other_err!("fake GFX", source: e))?);
    }
    Ok(Box::new(RawDvc(ironrdp_graphics::zgfx::wrap_uncompressed(&bytes))))
}

/// Arbitrary bytes as one message on the graphics channel (M9-2: malformed server input).
pub(crate) fn gfx_raw_message(bytes: Vec<u8>) -> DvcMessage {
    Box::new(RawDvc(bytes))
}

/// Records the clipboard callbacks of the server-side `Cliprdr`.
#[derive(Debug)]
pub(crate) struct FakeClipboardBackend {
    pub(crate) state: SharedState,
}

impl_as_any!(FakeClipboardBackend);

impl FakeClipboardBackend {
    fn push(&self, event: ClipEvent) {
        lock(&self.state).clip_events.push(event);
    }
}

impl CliprdrBackend for FakeClipboardBackend {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        self.push(ClipEvent::Ready);
    }

    fn on_request_format_list(&mut self) {}

    fn on_process_negotiated_capabilities(&mut self, _capabilities: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        let list = available_formats
            .iter()
            .map(|f| (f.id.value(), f.name.as_ref().map(|n| n.value().to_owned())))
            .collect();
        self.push(ClipEvent::ClientFormatList(list));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.push(ClipEvent::DataRequest(request.format.value()));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let data = (!response.is_error()).then(|| response.data().to_vec());
        self.push(ClipEvent::DataResponse(data));
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}
