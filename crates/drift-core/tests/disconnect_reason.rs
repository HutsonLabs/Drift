//! M0-5 Red: exhaustive `DisconnectReason::is_retryable` table (plan §3, M7-1).

use drift_core::DisconnectReason::{self, *};

/// Exhaustive by construction: adding a variant fails to compile until it is classified.
fn expected(r: &DisconnectReason) -> bool {
    match r {
        Network | TlsEof | ServerShutdown | Timeout => true,
        AuthFailed | RdstlsFailed(_) | CertMismatch => false,
        ProtocolError(_) | RedirectLoop | UserClosed | LoggedOffRemotely | LocalNetworkDenied => false,
    }
}

#[test]
fn retryability_table() {
    let table = [
        (Network, true),
        (TlsEof, true),
        (ServerShutdown, true),
        (Timeout, true),
        (AuthFailed, false),
        (RdstlsFailed(0x52E), false),
        (RdstlsFailed(0), false),
        (CertMismatch, false),
        (ProtocolError("bad pdu".into()), false),
        (RedirectLoop, false),
        (UserClosed, false),
        (LoggedOffRemotely, false),
        (LocalNetworkDenied, false),
    ];
    for (reason, retryable) in table {
        assert_eq!(reason.is_retryable(), retryable, "{reason:?}");
        assert_eq!(expected(&reason), retryable, "table disagrees with classification for {reason:?}");
    }
}
