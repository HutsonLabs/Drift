//! [`GfxClient`]: the `Microsoft::Windows::RDS::Graphics` DVC processor.

use drift_codec::{
    BgraTile, ClearCodec, ProgressiveCodec, TilePool, UncompressedFormat, decode_planar, decode_uncompressed,
};
use drift_core::video::H264Decoder;
use drift_core::{Bgra, Point, Rect, Size};
use ironrdp_core::{Decode as _, ReadCursor};
use ironrdp_dvc::{DvcClientProcessor, DvcMessage, DvcProcessor};
use ironrdp_egfx::pdu::{
    Avc420BitmapStream, CacheToSurfacePdu, CapabilitySet, Codec1Type, GfxPdu, PixelFormat, SolidFillPdu,
    SurfaceToCachePdu, SurfaceToSurfacePdu, WireToSurface1Pdu, WireToSurface2Pdu,
};
use ironrdp_graphics::zgfx;
use ironrdp_pdu::{PduResult, pdu_other_err};
use tracing::{debug, trace};

use crate::ack::AckOutbox;
use crate::caps::caps_advertise_pdu;
use crate::error::GfxError;
use crate::sink::FrameSink;
use crate::state::{GfxState, clip, ensure_within, point, rect};

/// Name of the graphics pipeline dynamic virtual channel.
pub const CHANNEL_NAME: &str = "Microsoft::Windows::RDS::Graphics";

/// Size of `RDPGFX_HEADER` (cmdId u16, flags u16, pduLength u32).
const GFX_HEADER_SIZE: usize = 8;

/// Keep at most this much decompression buffer between payloads (same as IronRDP).
const MAX_RETAINED_BUFFER: usize = 1 << 20;

/// Drift's RDPGFX client (plan §2 decision 2).
///
/// It decompresses `RDP_SEGMENTED_DATA`/ZGFX, decodes GFX PDUs with `ironrdp_egfx::pdu`,
/// tracks surfaces and cache slots, runs the codecs (AVC420 through the [`H264Decoder`] seam,
/// RFX Progressive, Planar, ClearCodec and Uncompressed through `drift-codec`) and drives a
/// [`FrameSink`]. Frame acknowledgements are produced from the sink's `presented` callbacks
/// into an [`AckOutbox`] (see the `ack` module docs for the policy).
///
/// Every server-supplied rectangle is validated before it reaches the sink. Any malformed,
/// inconsistent or unsupported input is a [`GfxError`], which the session maps to
/// `DisconnectReason::ProtocolError`; nothing on this path panics.
pub struct GfxClient {
    sink: Box<dyn FrameSink>,
    h264: Box<dyn H264Decoder>,
    progressive: ProgressiveCodec,
    clear: ClearCodec,
    zgfx: zgfx::Decompressor,
    buffer: Vec<u8>,
    state: GfxState,
    acks: AckOutbox,
    confirmed: Option<CapabilitySet>,
    output: Size<u32>,
    error: Option<GfxError>,
}

impl std::fmt::Debug for GfxClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GfxClient")
            .field("state", &self.state)
            .field("confirmed", &self.confirmed)
            .field("output", &self.output)
            .field("acks", &self.acks)
            .finish_non_exhaustive()
    }
}

impl GfxClient {
    /// Creates a client drawing into `sink`, decoding AVC420 with `h264` (`drift-video`'s
    /// VideoToolbox decoder in the app) and RFX Progressive tiles on `pool`.
    pub fn new(sink: Box<dyn FrameSink>, h264: Box<dyn H264Decoder>, pool: TilePool) -> Self {
        Self {
            sink,
            h264,
            progressive: ProgressiveCodec::new(pool),
            clear: ClearCodec::new(),
            zgfx: zgfx::Decompressor::new(),
            buffer: Vec::new(),
            state: GfxState::default(),
            acks: AckOutbox::default(),
            confirmed: None,
            output: Size::new(0, 0),
            error: None,
        }
    }

    /// Handle onto the frame acknowledgements this client produces. The session actor
    /// installs a notifier on it and sends drained acks on the graphics channel.
    pub fn acks(&self) -> AckOutbox {
        self.acks.clone()
    }

    /// Processes one DVC payload as received (`RDP_SEGMENTED_DATA`, ZGFX-compressed).
    ///
    /// # Errors
    /// Any [`GfxError`]; the session must then end with `ProtocolError`.
    pub fn process_payload(&mut self, payload: &[u8]) -> Result<(), GfxError> {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.clear();
        let result = match self.zgfx.decompress(payload, &mut buffer) {
            Ok(_) => self.process_pdus(&buffer),
            Err(e) => Err(GfxError::Zgfx(format!("{e:?}"))),
        };
        buffer.clear();
        buffer.shrink_to(MAX_RETAINED_BUFFER);
        self.buffer = buffer;
        result
    }

    /// Processes already-decompressed bytes holding one or more GFX PDUs back to back. Each
    /// PDU is decoded from exactly the `pduLength` bytes its header declares.
    ///
    /// # Errors
    /// Any [`GfxError`]; PDUs before the failing one have been applied.
    pub fn process_pdus(&mut self, mut data: &[u8]) -> Result<(), GfxError> {
        while !data.is_empty() {
            let len = data
                .get(4..GFX_HEADER_SIZE)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_le_bytes)
                .and_then(|l| usize::try_from(l).ok())
                .ok_or_else(|| GfxError::Decode(format!("truncated RDPGFX_HEADER ({} bytes)", data.len())))?;
            if len < GFX_HEADER_SIZE || len > data.len() {
                return Err(GfxError::Decode(format!(
                    "pduLength {len} outside {GFX_HEADER_SIZE}..={}",
                    data.len()
                )));
            }
            let (pdu_bytes, rest) = data.split_at(len);
            let pdu = GfxPdu::decode(&mut ReadCursor::new(pdu_bytes))
                .map_err(|e| GfxError::Decode(e.to_string()))?;
            self.handle_pdu(pdu)?;
            data = rest;
        }
        Ok(())
    }

    /// Applies one decoded GFX PDU.
    ///
    /// # Errors
    /// Any [`GfxError`]; a failing PDU never reaches the sink.
    pub fn handle_pdu(&mut self, pdu: GfxPdu) -> Result<(), GfxError> {
        trace!(pdu = pdu_name(&pdu), "GFX PDU");
        match pdu {
            GfxPdu::CapabilitiesConfirm(c) => {
                let caps = c.0.parsed().map_err(|e| GfxError::Decode(e.to_string()))?;
                debug!(?caps, "GFX capabilities confirmed");
                self.confirmed = caps;
            }
            GfxPdu::ResetGraphics(r) => self.reset_graphics(Size::new(r.width, r.height)),
            GfxPdu::CreateSurface(c) => {
                let size = Size::new(u32::from(c.width), u32::from(c.height));
                self.state.create(c.surface_id, size, c.pixel_format)?;
                // A reused id must not inherit progressive tiles from an earlier surface.
                self.progressive.delete_surface(c.surface_id);
                self.sink.create_surface(c.surface_id, size);
            }
            GfxPdu::DeleteSurface(d) => {
                if self.state.delete(d.surface_id) {
                    self.progressive.delete_surface(d.surface_id);
                    self.sink.delete_surface(d.surface_id);
                }
            }
            GfxPdu::MapSurfaceToOutput(m) => {
                self.state.surface(m.surface_id)?;
                self.sink
                    .map_surface_to_output(m.surface_id, Point::new(m.output_origin_x, m.output_origin_y));
            }
            GfxPdu::MapSurfaceToScaledOutput(m) => {
                // Not requested by Drift's caps; honour the placement, ignore the scaling.
                self.state.surface(m.surface_id)?;
                self.sink
                    .map_surface_to_output(m.surface_id, Point::new(m.output_origin_x, m.output_origin_y));
            }
            // RAIL window mappings: Drift never runs RemoteApp sessions.
            GfxPdu::MapSurfaceToWindow(_) | GfxPdu::MapSurfaceToScaledWindow(_) => {}
            GfxPdu::StartFrame(_) => self.progressive.begin_frame(),
            GfxPdu::EndFrame(e) => {
                self.progressive.end_frame();
                let presented = self.acks.frame_ended(e.frame_id);
                self.sink.end_frame(e.frame_id, presented);
            }
            GfxPdu::SolidFill(f) => self.solid_fill(&f)?,
            GfxPdu::SurfaceToSurface(s) => self.surface_to_surface(&s)?,
            GfxPdu::SurfaceToCache(s) => self.surface_to_cache(&s)?,
            GfxPdu::CacheToSurface(c) => self.cache_to_surface(&c)?,
            GfxPdu::EvictCacheEntry(e) => {
                if self.state.cache_evict(e.cache_slot)? {
                    self.sink.evict_cache(e.cache_slot);
                }
            }
            // Drift never sends CacheImportOffer, so there is nothing to import.
            GfxPdu::CacheImportReply(_) => {}
            GfxPdu::DeleteEncodingContext(d) => {
                self.progressive.delete_context(d.surface_id, d.codec_context_id)
            }
            GfxPdu::WireToSurface1(w) => self.wire_to_surface_1(&w)?,
            GfxPdu::WireToSurface2(w) => self.wire_to_surface_2(&w)?,
            GfxPdu::CapabilitiesAdvertise(_)
            | GfxPdu::FrameAcknowledge(_)
            | GfxPdu::QoeFrameAcknowledge(_)
            | GfxPdu::CacheImportOffer(_) => return Err(GfxError::UnexpectedPdu(pdu_name(&pdu))),
            // `GfxPdu` is non-exhaustive; a PDU type this build does not know is not ours to draw.
            _ => return Err(GfxError::UnexpectedPdu(pdu_name(&pdu))),
        }
        Ok(())
    }

    /// Shows (`true`) or hides (`false`) the session: forwards to the sink and switches the
    /// acknowledgement policy (hidden: `SUSPEND_FRAME_ACKNOWLEDGEMENT`, see the `ack` module).
    pub fn set_visible(&mut self, visible: bool) {
        self.acks.set_hidden(!visible);
        self.sink.set_visible(visible);
    }

    /// The capability set the server confirmed, once `CapabilitiesConfirm` arrived.
    pub fn confirmed_caps(&self) -> Option<&CapabilitySet> {
        self.confirmed.as_ref()
    }

    /// The output size from the last `ResetGraphics` (0×0 before the first one).
    pub fn output_size(&self) -> Size<u32> {
        self.output
    }

    /// Frames completed (`EndFrame` processed) since the channel opened.
    pub fn total_frames_decoded(&self) -> u32 {
        self.acks.total_decoded()
    }

    /// The error that failed the last [`DvcProcessor::process`] call, if any. The session
    /// actor turns it into `DisconnectReason::ProtocolError` (IronRDP only carries a generic
    /// PDU error through `ActiveStage`).
    pub fn take_error(&mut self) -> Option<GfxError> {
        self.error.take()
    }

    fn reset_graphics(&mut self, output: Size<u32>) {
        for id in self.state.reset() {
            self.progressive.delete_surface(id);
        }
        self.progressive.reset();
        self.clear = ClearCodec::new();
        self.h264.reset();
        self.output = output;
        self.sink.reset(output);
    }

    fn solid_fill(&mut self, f: &SolidFillPdu) -> Result<(), GfxError> {
        let surface = self.state.surface(f.surface_id)?;
        let mut rects = Vec::with_capacity(f.rectangles.len());
        for r in &f.rectangles {
            if let Some(r) = clip(rect(r)?, surface.size) {
                rects.push(r);
            }
        }
        let a = if surface.format == PixelFormat::ARgb { f.fill_pixel.xa } else { 0xFF };
        let color = Bgra::new(f.fill_pixel.b, f.fill_pixel.g, f.fill_pixel.r, a);
        if !rects.is_empty() {
            self.sink.solid_fill(f.surface_id, color, &rects);
        }
        Ok(())
    }

    fn surface_to_surface(&mut self, s: &SurfaceToSurfacePdu) -> Result<(), GfxError> {
        let src = self.state.surface(s.source_surface_id)?;
        let dst = self.state.surface(s.destination_surface_id)?;
        let r = rect(&s.source_rectangle)?;
        ensure_within(s.source_surface_id, src.size, r, "source")?;
        let dests = self.destinations(s.destination_surface_id, dst.size, r.size(), &s.destination_points)?;
        self.sink.surface_to_surface(s.source_surface_id, s.destination_surface_id, r, &dests);
        Ok(())
    }

    fn surface_to_cache(&mut self, s: &SurfaceToCachePdu) -> Result<(), GfxError> {
        let surface = self.state.surface(s.surface_id)?;
        let r = rect(&s.source_rectangle)?;
        ensure_within(s.surface_id, surface.size, r, "source")?;
        self.state.cache_store(s.cache_slot, r.size())?;
        self.sink.surface_to_cache(s.surface_id, r, s.cache_slot);
        Ok(())
    }

    fn cache_to_surface(&mut self, c: &CacheToSurfacePdu) -> Result<(), GfxError> {
        let size = self.state.cache_entry(c.cache_slot)?;
        let surface = self.state.surface(c.surface_id)?;
        let dests = self.destinations(c.surface_id, surface.size, size, &c.destination_points)?;
        self.sink.cache_to_surface(c.cache_slot, c.surface_id, &dests);
        Ok(())
    }

    /// Converts destination points, checking that an image of `size` fits at each of them.
    fn destinations(
        &self,
        surface: u16,
        bounds: Size<u32>,
        size: Size<u32>,
        points: &[ironrdp_egfx::pdu::Point],
    ) -> Result<Vec<Point<u32>>, GfxError> {
        points
            .iter()
            .map(|p| {
                let p = point(p);
                ensure_within(surface, bounds, Rect::new(p.x, p.y, size.width, size.height), "destination")?;
                Ok(p)
            })
            .collect()
    }

    fn wire_to_surface_1(&mut self, w: &WireToSurface1Pdu) -> Result<(), GfxError> {
        let surface = self.state.surface(w.surface_id)?;
        let dest = rect(&w.destination_rectangle)?;
        let tile = match w.codec_id {
            Codec1Type::Avc420 => return self.avc420(w.surface_id, surface.size, &w.bitmap_data),
            Codec1Type::Uncompressed => {
                ensure_within(w.surface_id, surface.size, dest, "destination")?;
                let format = match w.pixel_format {
                    PixelFormat::XRgb => UncompressedFormat::Xrgb,
                    PixelFormat::ARgb => UncompressedFormat::Argb,
                };
                decode_uncompressed(dest, format, &w.bitmap_data)?
            }
            Codec1Type::Planar => {
                ensure_within(w.surface_id, surface.size, dest, "destination")?;
                decode_planar(dest, &w.bitmap_data)?
            }
            Codec1Type::ClearCodec => {
                ensure_within(w.surface_id, surface.size, dest, "destination")?;
                self.clear.decode(dest, &w.bitmap_data)?
            }
            Codec1Type::RemoteFx
            | Codec1Type::Alpha
            | Codec1Type::Avc444
            | Codec1Type::Avc444v2
            | Codec1Type::DriftHevc => {
                return Err(GfxError::UnsupportedCodec(format!("{:?}", w.codec_id)));
            }
        };
        self.blit(w.surface_id, surface.size, &tile);
        Ok(())
    }

    fn wire_to_surface_2(&mut self, w: &WireToSurface2Pdu) -> Result<(), GfxError> {
        // `Codec2Type` has a single variant (RFX Progressive); anything else fails to decode.
        let surface = self.state.surface(w.surface_id)?;
        let tiles =
            self.progressive.decode(w.surface_id, w.codec_context_id, surface.size, &w.bitmap_data)?;
        for tile in &tiles {
            self.blit(w.surface_id, surface.size, tile);
        }
        Ok(())
    }

    /// `RFX_AVC420_BITMAP_STREAM`: the metablock's regions say which parts of the decoded
    /// picture changed; the rest is one Annex-B access unit for the H.264 decoder.
    fn avc420(&mut self, id: u16, bounds: Size<u32>, data: &[u8]) -> Result<(), GfxError> {
        let stream = Avc420BitmapStream::decode(&mut ReadCursor::new(data))
            .map_err(|e| GfxError::Decode(e.to_string()))?;
        let mut regions = Vec::with_capacity(stream.rectangles.len());
        for r in &stream.rectangles {
            if let Some(r) = clip(rect(r)?, bounds) {
                regions.push(r);
            }
        }
        // Always decode, even without visible regions: later pictures reference this one.
        if let Some(frame) = self.h264.decode(stream.data)?
            && !regions.is_empty()
        {
            self.sink.blit_nv12(id, &frame, &regions);
        }
        Ok(())
    }

    fn blit(&mut self, id: u16, bounds: Size<u32>, tile: &BgraTile) {
        for b in tile.blits() {
            // The codecs clip to the destination, which was checked against the surface;
            // keep the FrameSink promise even if a codec ever produced a stray rectangle.
            if b.rect.fits_within(bounds) {
                self.sink.blit_bgra(id, b.rect, b.stride, b.data);
            }
        }
    }
}

/// PDU name for logs and errors.
fn pdu_name(pdu: &GfxPdu) -> &'static str {
    match pdu {
        GfxPdu::WireToSurface1(_) => "WireToSurface1",
        GfxPdu::WireToSurface2(_) => "WireToSurface2",
        GfxPdu::DeleteEncodingContext(_) => "DeleteEncodingContext",
        GfxPdu::SolidFill(_) => "SolidFill",
        GfxPdu::SurfaceToSurface(_) => "SurfaceToSurface",
        GfxPdu::SurfaceToCache(_) => "SurfaceToCache",
        GfxPdu::CacheToSurface(_) => "CacheToSurface",
        GfxPdu::EvictCacheEntry(_) => "EvictCacheEntry",
        GfxPdu::CreateSurface(_) => "CreateSurface",
        GfxPdu::DeleteSurface(_) => "DeleteSurface",
        GfxPdu::StartFrame(_) => "StartFrame",
        GfxPdu::EndFrame(_) => "EndFrame",
        GfxPdu::FrameAcknowledge(_) => "FrameAcknowledge",
        GfxPdu::ResetGraphics(_) => "ResetGraphics",
        GfxPdu::MapSurfaceToOutput(_) => "MapSurfaceToOutput",
        GfxPdu::CacheImportOffer(_) => "CacheImportOffer",
        GfxPdu::CacheImportReply(_) => "CacheImportReply",
        GfxPdu::CapabilitiesAdvertise(_) => "CapabilitiesAdvertise",
        GfxPdu::CapabilitiesConfirm(_) => "CapabilitiesConfirm",
        GfxPdu::MapSurfaceToWindow(_) => "MapSurfaceToWindow",
        GfxPdu::QoeFrameAcknowledge(_) => "QoeFrameAcknowledge",
        GfxPdu::MapSurfaceToScaledOutput(_) => "MapSurfaceToScaledOutput",
        GfxPdu::MapSurfaceToScaledWindow(_) => "MapSurfaceToScaledWindow",
        _ => "unknown GFX PDU",
    }
}

ironrdp_core::impl_as_any!(GfxClient);

impl DvcProcessor for GfxClient {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    /// Sends exactly `[V8_1{AVC420_ENABLED}, V8{}]` (plan §1.4).
    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(vec![Box::new(caps_advertise_pdu()) as DvcMessage])
    }

    /// Processes one payload and returns the acks that are already due (a renderer that
    /// presents synchronously acks within the call; otherwise the actor drains
    /// [`GfxClient::acks`] when notified). On failure the [`GfxError`] is kept for
    /// [`GfxClient::take_error`].
    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        match self.process_payload(payload) {
            Ok(()) => Ok(self.acks.drain_messages()),
            Err(e) => {
                debug!(error = %e, "GFX protocol error");
                self.error = Some(e.clone());
                Err(pdu_other_err!("graphics pipeline", source: e))
            }
        }
    }

    fn close(&mut self, _channel_id: u32) {
        debug!("GFX channel closed");
    }
}

impl DvcClientProcessor for GfxClient {}
