//! What the host tells devices about itself (task **HOST-8**): its Bonjour advert and the
//! pairing link its QR code holds. The formats are the client's (`drift_core::pairing`, IOS-9);
//! `tests/advert.rs` checks they parse there.

use drift_core::CertFingerprint;
use drift_core::pairing::{PROTOCOL_VERSION, PairingPayload, PairingTarget, ShortCode};
use uuid::Uuid;

/// The Bonjour service type.
pub const SERVICE: &str = drift_core::pairing::SERVICE_TYPE;

/// This host, as devices should see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAnnouncement {
    /// The host's stable id.
    pub host_id: Uuid,
    /// The Mac's name.
    pub name: String,
    /// Its `.local` name.
    pub host: String,
    /// The RDP port.
    pub port: u16,
    /// SHA-256 of the host certificate.
    pub fingerprint: CertFingerprint,
    /// The account devices sign in with (HOST-15: filled in on the device).
    pub account: String,
}

impl HostAnnouncement {
    /// The TXT record: `v`, `id`, `name`, `host`, `port`, `fp` (64 hex digits), and since
    /// HOST-15 `user` (the account) and `approve=1` (devices may ask instead of typing a code).
    pub fn txt(&self) -> Vec<(String, String)> {
        let fp: String = self.fingerprint.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        vec![
            ("v".into(), PROTOCOL_VERSION.to_string()),
            ("id".into(), self.host_id.to_string()),
            ("name".into(), self.name.clone()),
            ("host".into(), self.host.clone()),
            ("port".into(), self.port.to_string()),
            ("fp".into(), fp),
            ("user".into(), self.account.clone()),
            ("approve".into(), "1".into()),
        ]
    }

    /// The `drift-pair:` link for `code`.
    pub fn pairing_link(&self, code: &ShortCode) -> String {
        PairingPayload::from(PairingTarget {
            host_id: self.host_id,
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            fingerprint: self.fingerprint,
            code: code.clone(),
        })
        .to_uri()
    }

    /// The pairing link as a QR code drawn with Unicode half blocks, for a terminal.
    pub fn pairing_qr(&self, code: &ShortCode) -> String {
        match qrcode::QrCode::new(self.pairing_link(code).as_bytes()) {
            Ok(qr) => qr
                .render::<qrcode::render::unicode::Dense1x2>()
                .dark_color(qrcode::render::unicode::Dense1x2::Light)
                .light_color(qrcode::render::unicode::Dense1x2::Dark)
                .build(),
            Err(e) => format!("(no QR code: {e})"),
        }
    }

    /// The pairing link's QR code as modules, for drawing it in a window (HOST-15): the width
    /// in modules and, row by row, whether each is dark. `None` if the link does not fit.
    pub fn pairing_qr_modules(&self, code: &ShortCode) -> Option<(usize, Vec<bool>)> {
        let qr = qrcode::QrCode::new(self.pairing_link(code).as_bytes()).ok()?;
        let dark = qr.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
        Some((qr.width(), dark))
    }

    /// Starts advertising on the local network; stops when the returned value is dropped.
    pub fn advertise(&self) -> Option<drift_apple::bonjour::Advertisement> {
        drift_apple::bonjour::Advertisement::register(&self.name, SERVICE, self.port, &self.txt())
    }
}

/// The Mac's `.local` host name (`gethostname`, with `.local` added when missing).
pub fn local_host_name() -> String {
    let mut buf = [0_u8; 256];
    // SAFETY: `buf` is valid for its length; gethostname writes a NUL-terminated name into it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let name = if ok {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::new()
    };
    match name.as_str() {
        "" => "localhost".into(),
        n if n.ends_with(".local") => n.to_owned(),
        n => format!("{}.local", n.split('.').next().unwrap_or(n)),
    }
}
