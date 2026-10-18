//! The session actor behind [`crate::spawn_session`].
//!
//! One Tokio task per tab. It connects leg after leg ([`crate::connect::connect_leg`]),
//! follows Server Redirections ([`crate::redirect::RedirectLoop`]), runs IronRDP's
//! `ActiveStage` while a leg is up, and drives everything that hangs off a live session:
//!
//! | Task | What the actor does |
//! |---|---|
//! | M1-1/M3-1 | connect, redirect loop, certificate prompts, graceful shutdown |
//! | M2-x | fast-path input, pointer updates → [`crate::SessionEvent::Cursor`] |
//! | M4-2 | [`crate::resize::ResizeDriver`] → Display Control, `ResetGraphics` → new desktop size |
//! | M5-2 | [`crate::clipboard`] ↔ `drift_clipboard::ClipboardSync` |
//! | M6-3 | Suppress Output and ack suspension while the tab is hidden |
//! | M7-3 | reconnect with `ReconnectPolicy`, mode-specific resume, `ReleaseAll` + `SyncToggles` |
//!
//! Every state change goes through `SessionState::transition`. Commands are received by a
//! small forwarder task so that `Close` (or dropping every [`SessionHandle`]) cancels a leg
//! that is still connecting.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use drift_clipboard::{ClipboardContents, SyncInput};
use drift_core::{
    CertFingerprint, Clock, ConnectStage, ConnectionProfile, DesktopSize, DisconnectReason,
    DisplayControlCaps, InputEvent, MonitorLayout, ReconnectDecision, ReconnectPolicy, SessionState, Size,
    ViewGeometry, layout::device_scale_for,
};
use drift_gfx::FrameSink;
use ironrdp_async::FramedWrite as _;
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::rdp::headers::ShareDataPdu;
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;
use ironrdp_pdu::rdp::suppress_output::SuppressOutputPdu;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason};
use tokio::sync::{mpsc, watch};

use crate::clipboard::{ClipOutcome, ClipboardChannel};
use crate::connect::{
    self, CertDecision, CertExpectation, ConnectObserver, ConnectedLeg, LegAuth, LegRequest, TokioDialer,
    UpgradedFramed,
};
use crate::fastpath::InputEncoder;
use crate::graphics::{self, Graphics};
use crate::greeter::GreeterTypist;
use crate::pointer::cursor_update;
use crate::redirect::RedirectLoop;
use crate::resize::{self, ResizeDriver};
use crate::session::{
    CertificateRole, SessionCapabilities, SessionCommand, SessionEvent, SessionEvents, SessionHandle,
    SessionOptions, SessionSecrets,
};
use crate::stats::StatsMeter;

/// How long `Close` waits for the server to acknowledge the Shutdown Request.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// How often a live leg checks its timers (debounce, greeter typing, statistics).
const TICK: Duration = Duration::from_millis(25);
/// How long the actor collects the app's initial view state before connecting leg 1.
const STARTUP_GRACE: Duration = Duration::from_millis(20);

/// Spawns the actor; see [`crate::spawn_session`].
pub(crate) fn spawn(
    profile: ConnectionProfile,
    secrets: SessionSecrets,
    frame_sink: Box<dyn FrameSink>,
    clock: Arc<dyn Clock>,
    options: SessionOptions,
) -> (SessionHandle, SessionEvents) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel::<SessionCommand>();
    let (closed_tx, closed_rx) = watch::channel(false);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            if cmd == SessionCommand::Close {
                let _ = closed_tx.send(true);
            }
            if fwd_tx.send(cmd).is_err() {
                return;
            }
        }
        // Every handle dropped: same as Close.
        let _ = closed_tx.send(true);
    });

    let seed = options.reconnect_seed.unwrap_or_else(random_seed);
    let policy = ReconnectPolicy::new(options.reconnect, seed);
    let resize = ResizeDriver::new(profile.mode, profile.display);
    let actor = Actor {
        graphics: Graphics::new(frame_sink, Arc::clone(&clock)),
        clock,
        io: Io { cmds: fwd_rx, events: ev_tx, state: SessionState::Idle, leg: 1, session_pin: None },
        closed: closed_rx,
        view: View::default(),
        resize,
        typist: GreeterTypist::new(),
        input: InputEncoder::default(),
        clipboard: Arc::new(Mutex::new(None)),
        clipboard_local: None,
        change_count: 0,
        policy,
        started: false,
        activations: 0,
        scale: 100,
        profile,
        secrets,
        options,
    };
    tokio::spawn(actor.run());
    (SessionHandle::from_sender(cmd_tx), ev_rx)
}

/// A seed for the backoff jitter; the exact value never matters, only that sessions differ.
fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or(0)
}

/// How a connected leg ended.
enum LegEnd {
    /// The server redirected the client.
    Redirect(Box<ServerRedirectionPdu>),
    /// The user closed the session.
    Closed,
    /// The leg failed or the server ended it.
    Ended(DisconnectReason),
}

/// The actor's channels and state; borrowed by the connect observer.
struct Io {
    cmds: mpsc::UnboundedReceiver<SessionCommand>,
    events: mpsc::UnboundedSender<SessionEvent>,
    state: SessionState,
    leg: u8,
    /// A certificate the user accepted in this session (without persisting it yet).
    session_pin: Option<CertFingerprint>,
}

impl Io {
    fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }

    fn set_state(&mut self, next: SessionState) {
        if self.state == next {
            return;
        }
        match self.state.transition(next) {
            Ok(next) => {
                self.state = next.clone();
                self.emit(SessionEvent::State(next));
            }
            Err(e) => tracing::warn!(error = %e, "rejected session state transition"),
        }
    }
}

impl ConnectObserver for Io {
    fn stage(&mut self, stage: ConnectStage) {
        self.set_state(SessionState::Connecting { leg: self.leg, stage });
    }

    async fn decide_certificate(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: CertFingerprint,
        role: CertificateRole,
    ) -> CertDecision {
        if self.session_pin == Some(fingerprint) {
            return CertDecision::Accept { pin: false };
        }
        self.emit(SessionEvent::CertificatePrompt { host: host.to_owned(), port, fingerprint, role });
        loop {
            match self.cmds.recv().await {
                Some(SessionCommand::AcceptCertificate { fingerprint: accepted, pin })
                    if accepted == fingerprint =>
                {
                    self.session_pin = Some(fingerprint);
                    if pin {
                        self.emit(SessionEvent::CertificatePinned(fingerprint));
                    }
                    return CertDecision::Accept { pin };
                }
                Some(SessionCommand::AcceptCertificate { .. }) => {
                    tracing::warn!("ignoring acceptance of a certificate that is not being prompted");
                }
                Some(SessionCommand::RejectCertificate) => return CertDecision::Reject,
                Some(SessionCommand::Close) | None => return CertDecision::Closed,
                Some(_) => {}
            }
        }
    }
}

/// What the app told us about the tab.
#[derive(Debug, Clone, Copy)]
struct View {
    visible: bool,
    focused: bool,
    geometry: Option<ViewGeometry>,
}

impl Default for View {
    fn default() -> Self {
        Self { visible: true, focused: false, geometry: None }
    }
}

/// The Display Control capabilities of the current leg, filled from the channel's callback.
type SharedDispCaps = Arc<Mutex<Option<DisplayControlCaps>>>;

struct Actor {
    profile: ConnectionProfile,
    secrets: SessionSecrets,
    /// The tab's frame sink and the GFX ack wake-up, shared by every leg.
    graphics: Graphics,
    clock: Arc<dyn Clock>,
    options: SessionOptions,
    io: Io,
    closed: watch::Receiver<bool>,
    view: View,
    resize: ResizeDriver,
    typist: GreeterTypist,
    /// Held keys and buttons; kept across legs so a reconnect can release them.
    input: InputEncoder,
    /// The current leg's clipboard state (the channel restarts with every connection).
    clipboard: Arc<Mutex<Option<ClipboardChannel>>>,
    /// The latest local pasteboard contents, replayed onto every new leg.
    clipboard_local: Option<ClipboardContents>,
    change_count: i64,
    policy: ReconnectPolicy,
    /// The initial view state has been collected (see `collect_initial_state`).
    started: bool,
    /// Legs activated so far (a reconnect resyncs input from the second one on).
    activations: u32,
    /// `DesktopScaleFactor` currently requested.
    scale: u32,
}

impl Actor {
    async fn run(mut self) {
        loop {
            let reason = self.connect_loop().await;
            // A Drift Host that refused this device closes the connection; say why, and do not
            // retry (HOST-8).
            let reason =
                match self.options.pairing.as_ref().and_then(drift_ext::pairing::ClientPairing::refusal) {
                    Some(why) if reason != DisconnectReason::UserClosed => {
                        DisconnectReason::PairingRefused(why)
                    }
                    _ => reason,
                };
            let end = match self.after_disconnect(&reason).await {
                Resume::Exit => return,
                Resume::Reconnect => continue,
                Resume::Stop(state) => state,
            };
            self.io.set_state(end);
            match self.idle().await {
                Resume::Reconnect => continue,
                Resume::Exit | Resume::Stop(_) => return,
            }
        }
    }

    /// What to do after a connection ended.
    async fn after_disconnect(&mut self, reason: &DisconnectReason) -> Resume {
        if *reason == DisconnectReason::UserClosed {
            self.io.set_state(SessionState::Disconnected { reason: reason.clone() });
            return Resume::Exit;
        }
        // A session that never came up reports the error instead of retrying behind the user's
        // back; the overlay's "Try again" sends `ReconnectNow`.
        if self.activations == 0 {
            return Resume::Stop(terminal_state(reason));
        }
        match self.policy.on_disconnect(reason, self.clock.now()) {
            ReconnectDecision::Retry { attempt, delay } => self.wait_backoff(attempt, delay, reason).await,
            ReconnectDecision::WaitForNetwork { attempt } => {
                self.wait_backoff(attempt, Duration::MAX, reason).await
            }
            ReconnectDecision::GiveUp(_) => Resume::Stop(terminal_state(reason)),
        }
    }

    /// Shows `Reconnecting` and waits for the backoff (or for the user / the network).
    async fn wait_backoff(&mut self, attempt: u32, delay: Duration, reason: &DisconnectReason) -> Resume {
        let shown = if delay == Duration::MAX { Duration::ZERO } else { delay };
        self.io.set_state(SessionState::Reconnecting { attempt, next_in: shown, reason: reason.clone() });
        let deadline = self.clock.now().checked_add(delay);
        loop {
            let ready = match deadline {
                Some(deadline) => self.clock.now() >= deadline,
                None => false,
            };
            if ready && self.policy.network_reachable() {
                return Resume::Reconnect;
            }
            let cmd = tokio::select! {
                cmd = self.io.cmds.recv() => cmd,
                () = tokio::time::sleep(TICK) => continue,
            };
            match cmd {
                None | Some(SessionCommand::Close) => {
                    self.io.set_state(SessionState::Disconnected { reason: DisconnectReason::UserClosed });
                    return Resume::Exit;
                }
                Some(SessionCommand::Cancel) => {
                    self.policy.reset();
                    return Resume::Stop(SessionState::Disconnected { reason: reason.clone() });
                }
                Some(SessionCommand::ReconnectNow) => {
                    if self.policy.network_reachable() {
                        return Resume::Reconnect;
                    }
                }
                Some(SessionCommand::NetworkReachable(reachable)) => {
                    self.policy.set_network_reachable(reachable);
                    if reachable {
                        return Resume::Reconnect;
                    }
                }
                Some(other) => self.remember(other),
            }
        }
    }

    /// Terminal state: stay addressable so the user can reconnect or close the tab.
    async fn idle(&mut self) -> Resume {
        while let Some(cmd) = self.io.cmds.recv().await {
            match cmd {
                SessionCommand::Close => break,
                SessionCommand::ReconnectNow => {
                    self.policy.reset();
                    self.policy.set_network_reachable(true);
                    return Resume::Reconnect;
                }
                SessionCommand::NetworkReachable(reachable) => {
                    self.policy.set_network_reachable(reachable);
                }
                other => self.remember(other),
            }
        }
        Resume::Exit
    }

    /// Records app state that must survive a disconnected phase.
    fn remember(&mut self, cmd: SessionCommand) {
        match cmd {
            SessionCommand::Resize(geometry) => {
                self.view.geometry = Some(geometry);
                self.resize.on_geometry(geometry, self.clock.now());
            }
            SessionCommand::SetVisible(visible) => self.view.visible = visible,
            SessionCommand::Focus(focused) => self.view.focused = focused,
            SessionCommand::ClipboardLocalChanged(contents) => self.clipboard_local = Some(contents),
            _ => {}
        }
    }

    fn tls_server_name(&self) -> String {
        self.options.tls_server_name.clone().unwrap_or_else(|| self.profile.host.clone())
    }

    fn first_leg(&mut self) -> LegRequest {
        let layout = self.resize.connect_layout();
        let desktop =
            layout.map_or(connect::DEFAULT_DESKTOP, |l| DesktopSize { width: l.width, height: l.height });
        self.scale = layout.map_or(100, |l| l.desktop_scale_factor);
        LegRequest {
            leg: 1,
            mode: self.profile.mode,
            host: self.profile.host.clone(),
            port: self.profile.port,
            tls_server_name: self.tls_server_name(),
            auth: LegAuth::Nla {
                username: self.profile.rdp_username.clone(),
                password: self.secrets.rdp_password.clone(),
            },
            expectation: self
                .profile
                .cert_pin
                .map_or(CertExpectation::TrustOnFirstUse, CertExpectation::Pinned),
            role: CertificateRole::Server,
            routing_token: None,
            client_name: self.options.client_name.clone(),
            desktop,
            scale: self.scale,
        }
    }

    /// Picks up the app's initial `Resize`/`Focus`/`SetVisible` before the first connect, so
    /// leg 1 already asks for the right desktop size (M4-2) and starts hidden if the tab is.
    async fn collect_initial_state(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let grace = tokio::time::sleep(STARTUP_GRACE);
        tokio::pin!(grace);
        loop {
            tokio::select! {
                () = &mut grace => return,
                cmd = self.io.cmds.recv() => match cmd {
                    None | Some(SessionCommand::Close) => return,
                    Some(other) => self.remember(other),
                },
            }
        }
    }

    /// Connects and runs legs until the session ends; returns why.
    async fn connect_loop(&mut self) -> DisconnectReason {
        self.collect_initial_state().await;
        let mut redirects =
            RedirectLoop::new(self.profile.mode, self.profile.host.clone(), self.profile.port);
        let mut request = self.first_leg();
        loop {
            self.io.leg = request.leg;
            let disp_caps: SharedDispCaps = Arc::default();
            let leg = match self.connect_one(request, &disp_caps).await {
                Ok(leg) => leg,
                Err(reason) => return reason,
            };
            let size = leg.result.desktop_size;
            let desktop = DesktopSize { width: u32::from(size.width), height: u32::from(size.height) };
            if let Some(state) = redirects.on_activated(desktop, self.scale) {
                self.io.set_state(state);
            }
            match self.run_leg(leg, &disp_caps).await {
                LegEnd::Closed => return DisconnectReason::UserClosed,
                LegEnd::Ended(reason) => return reason,
                LegEnd::Redirect(mut pdu) => {
                    let next = match redirects.on_redirect(&mut pdu) {
                        Ok(next) => next,
                        Err(reason) => return reason,
                    };
                    tracing::info!(leg = next.leg, "following Server Redirection");
                    let tls_server_name = if next.host == self.profile.host {
                        self.tls_server_name()
                    } else {
                        next.host.clone()
                    };
                    let expectation = match next.target_certificate {
                        Some(der) => CertExpectation::Exact(der),
                        None => self
                            .profile
                            .cert_pin
                            .or(self.io.session_pin)
                            .map_or(CertExpectation::TrustOnFirstUse, CertExpectation::Pinned),
                    };
                    let layout = self.resize.connect_layout();
                    let desktop = layout.map_or(connect::DEFAULT_DESKTOP, |l| DesktopSize {
                        width: l.width,
                        height: l.height,
                    });
                    self.scale = layout.map_or(100, |l| l.desktop_scale_factor);
                    request = LegRequest {
                        leg: next.leg,
                        mode: self.profile.mode,
                        host: next.host,
                        port: next.port,
                        tls_server_name,
                        auth: LegAuth::Rdstls(next.credentials),
                        expectation,
                        role: CertificateRole::RedirectTarget,
                        routing_token: next.routing_token,
                        client_name: self.options.client_name.clone(),
                        desktop,
                        scale: self.scale,
                    };
                }
            }
        }
    }

    /// The static channels of one leg: graphics, Display Control and (unless the profile turns
    /// the clipboard off) CLIPRDR.
    fn channels(
        &mut self,
        disp_caps: &SharedDispCaps,
    ) -> impl FnOnce(ironrdp_connector::ClientConnector) -> ironrdp_connector::ClientConnector + Send + use<>
    {
        let caps = Arc::clone(disp_caps);
        let display_control = DisplayControlClient::new(move |received| {
            let area = received.max_monitor_area();
            // Only the total area is exposed; keep it exactly, as one monitor.
            *caps.lock().unwrap_or_else(PoisonError::into_inner) = Some(DisplayControlCaps {
                max_num_monitors: 1,
                max_monitor_area_factor_a: u32::try_from(area).unwrap_or(u32::MAX),
                max_monitor_area_factor_b: 1,
            });
            Ok(Vec::new())
        });
        let channel =
            ClipboardChannel::new(self.profile.clipboard, self.view.focused, Arc::clone(&self.clock));
        let cliprdr = channel.client();
        *self.clipboard.lock().unwrap_or_else(PoisonError::into_inner) = Some(channel);
        let sound = self.options.audio.as_ref().map(crate::audio::AudioOutput::client);
        self.graphics.channels_for_leg(
            display_control,
            Some(cliprdr),
            self.options.drift_mac.as_ref(),
            self.options.pairing.as_ref(),
            self.options.gfx_caps,
            sound,
        )
    }

    async fn connect_one(
        &mut self,
        request: LegRequest,
        disp_caps: &SharedDispCaps,
    ) -> Result<ConnectedLeg, DisconnectReason> {
        if *self.closed.borrow() {
            return Err(DisconnectReason::UserClosed);
        }
        let timeout = self.options.connect_timeout;
        let clock = Arc::clone(&self.clock);
        let channels = self.channels(disp_caps);
        let closed = &mut self.closed;
        let io = &mut self.io;
        tokio::select! {
            res = connect::connect_leg(request, &TokioDialer, clock.as_ref(), timeout, io, channels) => res,
            () = wait_closed(closed) => Err(DisconnectReason::UserClosed),
        }
    }

    async fn run_leg(&mut self, leg: ConnectedLeg, disp_caps: &SharedDispCaps) -> LegEnd {
        let ConnectedLeg { framed, result, .. } = leg;
        let image =
            DecodedImage::new(PixelFormat::RgbA32, result.desktop_size.width, result.desktop_size.height);
        let clipboard_available = result.static_channels.get_channel_id_by_type::<CliprdrClient>().is_some();
        let stage = ActiveStageBuilder {
            static_channels: result.static_channels,
            user_channel_id: result.user_channel_id,
            io_channel_id: result.io_channel_id,
            message_channel_id: result.message_channel_id,
            share_id: result.share_id,
            compression_type: result.compression_type,
            enable_server_pointer: result.enable_server_pointer,
            pointer_software_rendering: result.pointer_software_rendering,
        }
        .build();
        let mut leg = Leg {
            framed,
            stage,
            image,
            desktop: DesktopSize {
                width: u32::from(result.desktop_size.width),
                height: u32::from(result.desktop_size.height),
            },
            stats: StatsMeter::new(self.clock.now()),
        };

        if let Some(end) = self.on_activated(&mut leg, clipboard_available).await {
            return end;
        }

        let acks_ready = self.graphics.acks_ready();
        let mut output = self.graphics.output_size();
        output.mark_unchanged();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                pdu = leg.framed.read_pdu() => {
                    let (action, payload) = match pdu {
                        Ok(pdu) => pdu,
                        Err(e) => return LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation)),
                    };
                    leg.stats.bytes(u64::try_from(payload.len()).unwrap_or(0));
                    // Frames completed while processing this payload are measured from here
                    // (M9-1: decode + present).
                    self.graphics.mark_wire(self.clock.now());
                    let outputs = match leg.stage.process(&mut leg.image, action, &payload) {
                        Ok(outputs) => outputs,
                        Err(e) => {
                            let reason = graphics::take_gfx_error(&mut leg.stage)
                                .unwrap_or_else(|| DisconnectReason::ProtocolError(e.to_string()));
                            return LegEnd::Ended(reason);
                        }
                    };
                    if let Some(end) = self.handle_outputs(&mut leg, outputs).await {
                        return end;
                    }
                    if let Some(end) = self.after_channel_io(&mut leg, disp_caps).await {
                        return end;
                    }
                }
                () = acks_ready.notified() => match graphics::flush_acks(&leg.stage) {
                    Ok(Some(frame)) => {
                        if let Err(e) = leg.framed.write_all(&frame).await {
                            return LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation));
                        }
                    }
                    Ok(None) => {}
                    Err(reason) => return LegEnd::Ended(reason),
                },
                changed = output.changed() => {
                    if changed.is_ok()
                        && let Some(size) = *output.borrow_and_update()
                    {
                        self.on_output_size(&mut leg, size);
                    }
                }
                cmd = self.io.cmds.recv() => {
                    if let Some(end) = self.on_command(&mut leg, cmd).await {
                        return end;
                    }
                }
                _ = ticker.tick() => {
                    if let Some(end) = self.on_tick(&mut leg).await {
                        return end;
                    }
                }
            }
        }
    }

    /// Everything a freshly activated leg needs.
    async fn on_activated(&mut self, leg: &mut Leg, clipboard: bool) -> Option<LegEnd> {
        let now = self.clock.now();
        self.policy.on_connected(now);
        self.resize.on_leg_activated(MonitorLayout {
            width: leg.desktop.width,
            height: leg.desktop.height,
            desktop_scale_factor: self.scale,
            device_scale_factor: device_scale_for(self.scale),
        });
        self.io.emit(SessionEvent::Capabilities(SessionCapabilities {
            display_control: resize::mode_has_display_control(self.profile.mode),
            clipboard,
            scale_mode: resize::scale_mode(self.profile.mode, self.profile.display),
        }));
        if self.io.state == SessionState::AwaitingGreeterLogin {
            self.typist.on_greeter(self.secrets.linux_password.is_some());
        } else {
            self.typist.disarm();
        }
        // Plan M7-3: after a reconnect (or a redirect) the remote has no idea what is held.
        if self.activations > 0 {
            let toggles = self.input.toggles();
            let mut events = self.input.encode(InputEvent::ReleaseAll);
            events.extend(self.input.encode(toggles));
            if let Some(end) = self.send_input_events(leg, events).await {
                return Some(end);
            }
        }
        self.activations += 1;
        if !self.view.visible
            && let Some(end) = self.apply_visibility(leg, false).await
        {
            return Some(end);
        }
        if let Some(contents) = self.clipboard_local.clone() {
            self.change_count += 1;
            let change_count = self.change_count;
            let actions =
                self.with_clipboard(|c| c.handle(SyncInput::LocalChanged { change_count, contents }));
            if let Some(end) = self.apply_clipboard(leg, actions).await {
                return Some(end);
            }
        }
        leg.stats.restart(now);
        None
    }

    /// The remote desktop size changed (`ResetGraphics` after a layout, plan §1.4).
    fn on_output_size(&mut self, leg: &mut Leg, size: Size<u32>) {
        leg.desktop = DesktopSize { width: size.width, height: size.height };
        if matches!(self.io.state, SessionState::Connected { .. }) {
            self.io.set_state(SessionState::Connected { desktop: leg.desktop, scale: self.scale });
        }
    }

    /// Timers: resize debounce, greeter typing, clipboard timeouts, statistics.
    async fn on_tick(&mut self, leg: &mut Leg) -> Option<LegEnd> {
        let now = self.clock.now();
        if let Some(layout) = self.resize.poll(now) {
            self.scale = layout.desktop_scale_factor;
            match leg.stage.encode_resize(
                layout.width,
                layout.height,
                Some(layout.desktop_scale_factor),
                None,
            ) {
                Some(Ok(frame)) => {
                    tracing::debug!(?layout, "requesting a monitor layout");
                    if let Err(e) = leg.framed.write_all(&frame).await {
                        return Some(LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation)));
                    }
                }
                Some(Err(e)) => tracing::warn!(error = %e, "cannot encode the monitor layout"),
                None => tracing::debug!("Display Control is not ready for a resize"),
            }
        }
        if let Some(password) = self.secrets.linux_password.clone()
            && let Some(events) = self.typist.poll(now, &password)
        {
            tracing::info!("typing the stored Linux password at the greeter");
            let events: Vec<_> = events.into_iter().flat_map(|e| self.input.encode(e)).collect();
            if let Some(end) = self.send_input_events(leg, events).await {
                return Some(end);
            }
        }
        let actions = self.with_clipboard(|c| c.handle(SyncInput::Tick));
        if let Some(end) = self.apply_clipboard(leg, actions).await {
            return Some(end);
        }
        leg.stats.frames(self.graphics.take_presented_frames());
        leg.stats.frame_latencies(self.graphics.take_frame_latencies());
        if let Some(stats) = leg.stats.sample(now, Graphics::unacked_frames(&leg.stage)) {
            self.io.emit(SessionEvent::Stats(stats));
        }
        None
    }

    /// One command from the app while a leg is up.
    async fn on_command(&mut self, leg: &mut Leg, cmd: Option<SessionCommand>) -> Option<LegEnd> {
        let now = self.clock.now();
        match cmd {
            None | Some(SessionCommand::Close) => {
                shutdown(&mut leg.framed, &mut leg.stage, &mut leg.image, self.clock.as_ref()).await;
                Some(LegEnd::Closed)
            }
            Some(SessionCommand::Input(event)) => {
                self.typist.on_input(&event, now);
                let events = self.input.encode(event);
                let end = self.send_input_events(leg, events).await;
                // M9-1: input-to-wire is the whole path, encoding included, up to the write.
                leg.stats.input_latency(self.clock.now().saturating_duration_since(now));
                end
            }
            Some(SessionCommand::Resize(geometry)) => {
                self.view.geometry = Some(geometry);
                self.resize.on_geometry(geometry, now);
                None
            }
            Some(SessionCommand::SetVisible(visible)) => {
                if self.view.visible == visible {
                    return None;
                }
                self.view.visible = visible;
                self.apply_visibility(leg, visible).await
            }
            Some(SessionCommand::Focus(focused)) => {
                self.view.focused = focused;
                let actions = self.with_clipboard(|c| c.handle(SyncInput::Focus(focused)));
                self.apply_clipboard(leg, actions).await
            }
            Some(SessionCommand::ClipboardLocalChanged(contents)) => {
                self.clipboard_local = Some(contents.clone());
                self.change_count += 1;
                let change_count = self.change_count;
                let actions =
                    self.with_clipboard(|c| c.handle(SyncInput::LocalChanged { change_count, contents }));
                self.apply_clipboard(leg, actions).await
            }
            Some(SessionCommand::NetworkReachable(reachable)) => {
                self.policy.set_network_reachable(reachable);
                None
            }
            // Certificate answers, Cancel and ReconnectNow have no meaning on a live session.
            Some(_) => None,
        }
    }

    /// Suppress Output plus the ack policy (M6-3).
    async fn apply_visibility(&mut self, leg: &mut Leg, visible: bool) -> Option<LegEnd> {
        if visible {
            // Resume acknowledgement first, so the full frame the server sends is acked.
            self.graphics.set_visible(&mut leg.stage, true);
        }
        let rect = visible.then(|| InclusiveRectangle {
            left: 0,
            top: 0,
            right: u16::try_from(leg.desktop.width.saturating_sub(1)).unwrap_or(u16::MAX),
            bottom: u16::try_from(leg.desktop.height.saturating_sub(1)).unwrap_or(u16::MAX),
        });
        let mut buf = ironrdp_core::WriteBuf::new();
        let pdu = ShareDataPdu::SuppressOutput(SuppressOutputPdu { desktop_rect: rect });
        match leg.stage.encode_static(&mut buf, pdu) {
            Ok(_) => {
                if let Err(e) = leg.framed.write_all(buf.filled()).await {
                    return Some(LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation)));
                }
            }
            Err(e) => tracing::warn!(error = %e, "cannot encode Suppress Output"),
        }
        if !visible {
            self.graphics.set_visible(&mut leg.stage, false);
        }
        None
    }

    async fn send_input_events(&mut self, leg: &mut Leg, events: Vec<InputEventPdu>) -> Option<LegEnd> {
        if events.is_empty() {
            return None;
        }
        match leg.stage.process_fastpath_input(&mut leg.image, &events) {
            Ok(outputs) => self.handle_outputs(leg, outputs).await,
            Err(e) => {
                tracing::warn!(error = %e, "dropping unencodable input");
                None
            }
        }
    }

    /// Runs a closure on the current leg's clipboard state.
    fn with_clipboard<T: Default>(&self, f: impl FnOnce(&mut ClipboardChannel) -> T) -> T {
        let mut guard = self.clipboard.lock().unwrap_or_else(PoisonError::into_inner);
        guard.as_mut().map(f).unwrap_or_default()
    }

    /// Executes clipboard actions: frames go on the wire, pasteboard writes become events.
    async fn apply_clipboard(
        &mut self,
        leg: &mut Leg,
        actions: Vec<drift_clipboard::SyncAction>,
    ) -> Option<LegEnd> {
        for action in actions {
            match crate::clipboard::apply(&mut leg.stage, action) {
                Ok(Some(ClipOutcome::Frame(frame))) => {
                    if let Err(e) = leg.framed.write_all(&frame).await {
                        return Some(LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation)));
                    }
                }
                Ok(Some(ClipOutcome::Write(contents))) => {
                    self.io.emit(SessionEvent::ClipboardRemote(contents));
                }
                Ok(None) => {}
                Err(reason) => return Some(LegEnd::Ended(reason)),
            }
        }
        None
    }

    /// Clipboard callbacks and Display Control capabilities produced by the last `process`.
    async fn after_channel_io(&mut self, leg: &mut Leg, disp_caps: &SharedDispCaps) -> Option<LegEnd> {
        if let Some(caps) = disp_caps.lock().unwrap_or_else(PoisonError::into_inner).take() {
            self.resize.on_display_control_ready(caps, self.clock.now());
        }
        let actions = self.with_clipboard(ClipboardChannel::drain);
        self.apply_clipboard(leg, actions).await
    }

    /// Writes response frames and turns terminal outputs into a [`LegEnd`].
    async fn handle_outputs(&mut self, leg: &mut Leg, outputs: Vec<ActiveStageOutput>) -> Option<LegEnd> {
        for output in outputs {
            if let Some(cursor) = cursor_update(&output, self.scale) {
                self.io.emit(SessionEvent::Cursor(cursor));
                continue;
            }
            match output {
                ActiveStageOutput::ResponseFrame(frame) => {
                    if let Err(e) = leg.framed.write_all(&frame).await {
                        return Some(LegEnd::Ended(connect::classify_io_error(&e, ConnectStage::Activation)));
                    }
                }
                ActiveStageOutput::ServerRedirect(pdu) => return Some(LegEnd::Redirect(pdu)),
                ActiveStageOutput::Terminate(reason) => {
                    return Some(LegEnd::Ended(match reason {
                        GracefulDisconnectReason::UserInitiated => DisconnectReason::LoggedOffRemotely,
                        GracefulDisconnectReason::ServerInitiated | GracefulDisconnectReason::Other(_) => {
                            DisconnectReason::ServerShutdown
                        }
                    }));
                }
                _ => {}
            }
        }
        None
    }
}

/// Fast-path input events of the wire encoder.
type InputEventPdu = ironrdp_pdu::input::fast_path::FastPathInputEvent;

/// What `run` does after a connection ended.
enum Resume {
    /// Connect again.
    Reconnect,
    /// Enter this terminal state and wait for the user.
    Stop(SessionState),
    /// The actor is done.
    Exit,
}

/// The terminal state for a reason: retryable reasons leave the door open.
fn terminal_state(reason: &DisconnectReason) -> SessionState {
    if reason.is_retryable() {
        SessionState::Disconnected { reason: reason.clone() }
    } else {
        SessionState::Failed { reason: reason.clone() }
    }
}

/// Everything that belongs to the currently connected leg.
struct Leg {
    framed: UpgradedFramed,
    stage: ActiveStage,
    image: DecodedImage,
    desktop: DesktopSize,
    stats: StatsMeter,
}

/// Graceful shutdown: Shutdown Request, then wait briefly for the server's answer.
async fn shutdown(
    framed: &mut UpgradedFramed,
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
    clock: &dyn Clock,
) {
    let Ok(outputs) = stage.graceful_shutdown() else { return };
    for output in outputs {
        if let ActiveStageOutput::ResponseFrame(frame) = output
            && framed.write_all(&frame).await.is_err()
        {
            return;
        }
    }
    let wait = async {
        while let Ok((action, payload)) = framed.read_pdu().await {
            match stage.process(image, action, &payload) {
                Ok(outputs) => {
                    for output in outputs {
                        if matches!(output, ActiveStageOutput::Terminate(_)) {
                            return;
                        }
                    }
                }
                Err(_) => return,
            }
        }
    };
    let deadline = clock.now() + SHUTDOWN_GRACE;
    // The real-time cap keeps a frozen test clock from holding the actor forever.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE * 2, connect::until(clock, deadline, wait)).await;
}

async fn wait_closed(closed: &mut watch::Receiver<bool>) {
    let _ = closed.wait_for(|c| *c).await;
}
