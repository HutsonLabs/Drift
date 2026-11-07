//! Fuzz target: the Server Redirection path (plan §1.3, tasks M3-1 / M9-2).
//!
//! Leg 1 of Remote Login ends with a server-controlled Server Redirection PDU that carries
//! lengths for a load-balance cookie, a UTF-16 user name, an opaque password blob, a GUID and
//! a 3 KiB certificate container — every one of them attacker-controlled before Drift has
//! authenticated anything. The target drives the same two steps the actor does:
//!
//! 1. `ShareControlPdu::ServerRedirect` decoding inside an X.224 frame (and, for shorter
//!    inputs, the bare `ServerRedirectionPdu`);
//! 2. `RedirectLoop::on_redirect`, which parses the routing token and the certificate
//!    container, extracts the one-time credentials and zeroizes the PDU.
//!
//! Errors are expected; a panic, an overflow or an OOM is a bug. Seed the corpus with
//! `fixtures/pdus/server_redirection_leg{1,2}.bin` (sanitized captures).
#![no_main]

use drift_core::ConnectMode;
use drift_rdp::redirect::{self, RedirectLoop};
use ironrdp_pdu::mcs::SendDataIndication;
use ironrdp_pdu::rdp::headers::{ShareControlHeader, ShareControlPdu};
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;
use ironrdp_pdu::x224::X224;
use libfuzzer_sys::fuzz_target;

/// Follows one decoded redirection PDU exactly as `SessionActor::connect_loop` does.
fn follow(pdu: ServerRedirectionPdu) {
    let mut loop_state = RedirectLoop::new(ConnectMode::RemoteLogin, "10.0.2.40", 3389);
    // Up to MAX_REDIRECTS + 1 so the loop protection itself is exercised.
    for _ in 0..6 {
        match loop_state.on_redirect(&mut pdu.clone()) {
            Ok(next) => {
                if let Some(der) = next.target_certificate.as_deref() {
                    let _ = redirect::verify_target_certificate(der, der);
                }
                std::hint::black_box(next.credentials.username().len());
            }
            Err(_) => break,
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // The wire shape: an X.224 frame carrying a Share Control PDU of type 0xA.
    if let Ok(X224(sdi)) = ironrdp_core::decode::<X224<SendDataIndication<'_>>>(data)
        && let Ok(header) = ironrdp_core::decode::<ShareControlHeader>(sdi.user_data.as_ref())
        && let ShareControlPdu::ServerRedirect(pdu) = header.share_control_pdu
    {
        follow(pdu);
    }
    // The bare PDU, so shallow inputs still reach the redirection parser itself.
    if let Ok(pdu) = ironrdp_core::decode::<ServerRedirectionPdu>(data) {
        follow(pdu);
    }
    // The routing-token parser takes raw load-balance info.
    let _ = redirect::routing_token(data);
});
