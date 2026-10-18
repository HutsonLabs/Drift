//! The host's clipboard: the CLIPRDR server role (task **HOST-11**, ADR
//! `HOST-11-clipboard-and-audio`).
//!
//! Decisions come from `drift-clipboard`'s direction-agnostic [`ClipboardSync`] (ADRs M5-1,
//! M5-3), the same state the client runs: newest wins, echo suppression of our own pasteboard
//! writes, size caps, text and images. This module only moves values:
//!
//! * IronRDP's server calls the per-connection [`HostClipBackend`], which forwards each callback
//!   to one **clipboard thread**;
//! * the clipboard thread owns the pasteboard (made by a [`PasteboardFactory`] on that thread,
//!   since `NSPasteboard` is not `Send`), polls its `changeCount` every
//!   [`drift_clipboard::poll::POLL_INTERVAL`], runs the sync state and sends the resulting PDUs
//!   to the session as `ServerEvent::Clipboard`.
//!
//! The host is always "focused": there is one session, and it is the user's.

use std::collections::VecDeque;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use drift_clipboard::formats::ClipFormat;
use drift_clipboard::poll::{POLL_INTERVAL, PasteboardPort, PasteboardWatcher};
use drift_clipboard::{ClipboardSync, SyncAction, SyncInput};
use drift_core::{ClipboardPrefs, SystemClock};
use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend, CliprdrBackendFactory};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_core::impl_as_any;
use ironrdp_server::{CliprdrServerFactory, ServerEvent, ServerEventSender};
use tokio::sync::mpsc::UnboundedSender;

/// Makes the host's pasteboard, on the clipboard thread.
pub type PasteboardFactory = Box<dyn Fn() -> Box<dyn PasteboardPort> + Send + Sync>;

/// What the clipboard thread is told.
enum ClipEvent {
    /// A new connection: start a fresh sync state (CLIPRDR restarts with every connection).
    Reset,
    /// Where PDUs go from now on.
    Sender(UnboundedSender<ServerEvent>),
    /// The client's first Format List arrived: the channel is ready.
    Ready,
    /// The client copied.
    RemoteCopy(Vec<ClipFormat>),
    /// The client wants our data in this format.
    DataRequest(u32),
    /// The client answered our request (`None` = error response).
    DataResponse(Option<Vec<u8>>),
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The CLIPRDR server factory handed to `ironrdp-server`; owns the clipboard thread.
pub struct HostClipboard {
    tx: Mutex<mpsc::Sender<ClipEvent>>,
    ready: Arc<Mutex<bool>>,
}

impl std::fmt::Debug for HostClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostClipboard").finish_non_exhaustive()
    }
}

impl HostClipboard {
    /// Starts the clipboard thread over the pasteboard `factory` makes, for every connection.
    pub fn start(factory: PasteboardFactory) -> std::io::Result<Self> {
        Self::start_gated(factory, crate::gate::always_authorized())
    }

    /// Starts the clipboard thread; nothing is exchanged with a connection until `authorized`
    /// says so (HOST-8): its copies are ignored, ours are not announced, its requests are refused.
    pub fn start_gated(
        factory: PasteboardFactory,
        authorized: crate::gate::Authorized,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let ready = Arc::new(Mutex::new(false));
        let thread_ready = Arc::clone(&ready);
        std::thread::Builder::new()
            .name("drift-host-clipboard".into())
            .spawn(move || run(factory(), &rx, &thread_ready, &*authorized))?;
        Ok(Self { tx: Mutex::new(tx), ready })
    }

    /// Whether the current connection's clipboard channel is ready (for stats and tests).
    pub fn ready_flag(&self) -> Arc<Mutex<bool>> {
        Arc::clone(&self.ready)
    }

    fn send(&self, event: ClipEvent) {
        let _ = lock(&self.tx).send(event);
    }
}

impl ServerEventSender for HostClipboard {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        self.send(ClipEvent::Sender(sender));
    }
}

impl CliprdrBackendFactory for HostClipboard {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        self.send(ClipEvent::Reset);
        Box::new(HostClipBackend { tx: lock(&self.tx).clone() })
    }
}

impl CliprdrServerFactory for HostClipboard {}

/// One connection's CLIPRDR backend: forwards callbacks, decides nothing.
#[derive(Debug)]
pub struct HostClipBackend {
    tx: mpsc::Sender<ClipEvent>,
}

impl_as_any!(HostClipBackend);

impl std::fmt::Debug for ClipEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the payloads: clipboard data may be anything the user copied.
        f.write_str(match self {
            Self::Reset => "Reset",
            Self::Sender(_) => "Sender",
            Self::Ready => "Ready",
            Self::RemoteCopy(_) => "RemoteCopy",
            Self::DataRequest(_) => "DataRequest",
            Self::DataResponse(_) => "DataResponse",
        })
    }
}

impl CliprdrBackend for HostClipBackend {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        let _ = self.tx.send(ClipEvent::Ready);
    }

    fn on_request_format_list(&mut self) {}

    fn on_process_negotiated_capabilities(&mut self, _capabilities: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        let formats = available_formats.iter().map(from_ironrdp).collect();
        let _ = self.tx.send(ClipEvent::RemoteCopy(formats));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let _ = self.tx.send(ClipEvent::DataRequest(request.format.value()));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let data = (!response.is_error()).then(|| response.data().to_vec());
        let _ = self.tx.send(ClipEvent::DataResponse(data));
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

fn to_ironrdp(format: &ClipFormat) -> ClipboardFormat {
    let out = ClipboardFormat::new(ClipboardFormatId(format.id));
    match &format.name {
        Some(name) => out.with_name(ClipboardFormatName::new(name.clone())),
        None => out,
    }
}

fn from_ironrdp(format: &ClipboardFormat) -> ClipFormat {
    ClipFormat { id: format.id.value(), name: format.name.as_ref().map(|n| n.value().to_owned()) }
}

/// The CLIPRDR messages for one sync action; pasteboard writes are done here and reported back.
pub fn messages_for(action: SyncAction) -> Option<ClipboardMessage> {
    Some(match action {
        SyncAction::SendFormatList(formats) => {
            ClipboardMessage::SendInitiateCopy(formats.iter().map(to_ironrdp).collect())
        }
        SyncAction::RequestRemoteData(format) => {
            ClipboardMessage::SendInitiatePaste(ClipboardFormatId(format.id))
        }
        SyncAction::SendData { data, format_id } => ClipboardMessage::SendFormatData(match data {
            Ok(bytes) => OwnedFormatDataResponse::new_data(bytes),
            Err(e) => {
                tracing::warn!(error = %e, format_id, "answering a clipboard request with an error");
                OwnedFormatDataResponse::new_error()
            }
        }),
        SyncAction::WritePasteboard(_) | SyncAction::Rejected(_) => return None,
    })
}

/// The clipboard thread: one sync state per connection, the pasteboard poll, the PDUs out.
fn run(
    port: Box<dyn PasteboardPort>,
    rx: &mpsc::Receiver<ClipEvent>,
    ready: &Mutex<bool>,
    authorized: &(dyn Fn() -> bool + Send + Sync),
) {
    let prefs = ClipboardPrefs::TextAndImages;
    let fresh = || ClipboardSync::new(prefs, true, Arc::new(SystemClock));
    let mut sync = fresh();
    let mut watcher = PasteboardWatcher::new(port.change_count());
    let mut sender: Option<UnboundedSender<ServerEvent>> = None;
    // The client's channel came up before its device was authorized: start once it is.
    let mut ready_pending = false;
    let send = |sender: &Option<UnboundedSender<ServerEvent>>, message: ClipboardMessage| {
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(ServerEvent::Clipboard(message));
        }
    };
    loop {
        let event = rx.recv_timeout(POLL_INTERVAL);
        let allowed = authorized();
        let input = match event {
            Ok(ClipEvent::Reset) => {
                sync = fresh();
                ready_pending = false;
                *lock(ready) = false;
                continue;
            }
            Ok(ClipEvent::Sender(s)) => {
                sender = Some(s);
                continue;
            }
            Ok(ClipEvent::Ready) if !allowed => {
                ready_pending = true;
                continue;
            }
            Ok(ClipEvent::Ready) => {
                *lock(ready) = true;
                SyncInput::InitialFormatListRequested
            }
            // Nothing from or to an unauthorized connection; its requests get an error answer.
            Ok(ClipEvent::DataRequest(_)) if !allowed => {
                send(&sender, ClipboardMessage::SendFormatData(OwnedFormatDataResponse::new_error()));
                continue;
            }
            Ok(ClipEvent::RemoteCopy(_) | ClipEvent::DataResponse(_)) if !allowed => continue,
            Ok(ClipEvent::RemoteCopy(formats)) => SyncInput::RemoteFormatList(formats),
            Ok(ClipEvent::DataRequest(format_id)) => SyncInput::RemoteDataRequest { format_id },
            Ok(ClipEvent::DataResponse(data)) => SyncInput::RemoteData(data),
            Err(RecvTimeoutError::Timeout) if !allowed => continue,
            Err(RecvTimeoutError::Timeout) if ready_pending => {
                ready_pending = false;
                *lock(ready) = true;
                SyncInput::InitialFormatListRequested
            }
            Err(RecvTimeoutError::Timeout) => match watcher.poll(&*port, prefs) {
                Some(change) => {
                    SyncInput::LocalChanged { change_count: change.change_count, contents: change.contents }
                }
                None => SyncInput::Tick,
            },
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut queue: VecDeque<SyncAction> = sync.handle(input).into();
        while let Some(action) = queue.pop_front() {
            match action {
                SyncAction::WritePasteboard(contents) => {
                    let change_count = port.write(&contents);
                    // Our own write must not come back as a host copy.
                    watcher = PasteboardWatcher::new(change_count);
                    queue.extend(sync.handle(SyncInput::LocalWritten { change_count }));
                }
                SyncAction::Rejected(e) => tracing::warn!(error = %e, "clipboard payload refused"),
                other => {
                    if let Some(message) = messages_for(other) {
                        send(&sender, message);
                    }
                }
            }
        }
    }
}
