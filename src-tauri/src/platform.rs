//! Thin platform glue used by commands (humble object; no logic).

use crate::profiles::CommandError;

/// Opens `url` with Launch Services (`x-apple.systempreferences:` URLs open System Settings).
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) -> Result<(), CommandError> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Err(CommandError::Platform { message: format!("invalid URL {url}") });
    };
    if NSWorkspace::sharedWorkspace().openURL(&nsurl) {
        Ok(())
    } else {
        Err(CommandError::Platform { message: format!("macOS could not open {url}") })
    }
}

/// Opens the place where the user grants Local Network access: System Settings › Privacy &
/// Security › Local Network on macOS, the app's own page in Settings on iOS (ADR IOS-3).
pub fn open_local_network_settings() -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    {
        open_url(drift_core::messages::LOCAL_NETWORK_SETTINGS_URL)
    }
    #[cfg(target_os = "ios")]
    {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return Err(CommandError::Platform {
                message: "Settings can only be opened from the main thread".into(),
            });
        };
        drift_ios::settings::open_app_settings(mtm)
            .map_err(|e| CommandError::Platform { message: e.to_string() })
    }
}

/// Scans a QR code with the camera over `window` and returns its text (IOS-9).
#[cfg(target_os = "ios")]
pub async fn scan_qr<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> Result<String, CommandError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |platform| {
            let Some(mtm) = objc2::MainThreadMarker::new() else { return };
            // SAFETY: Tauri passes its live view controller on the main thread.
            unsafe {
                drift_ios::qr::scan(mtm, platform.view_controller(), move |result| {
                    let _ = tx.send(result);
                })
            };
        })
        .map_err(|e| CommandError::Platform { message: e.to_string() })?;
    match rx.await {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(CommandError::Pairing { message: e.to_string() }),
        Err(_) => Err(CommandError::Pairing { message: "The scanner closed.".into() }),
    }
}

/// A Mac has no camera flow for pairing: the code is typed or the QR text pasted.
#[cfg(target_os = "macos")]
pub async fn scan_qr<R: tauri::Runtime>(_window: &tauri::WebviewWindow<R>) -> Result<String, CommandError> {
    Err(CommandError::Pairing {
        message: "Type the code shown on the Mac, or paste its pairing link.".into(),
    })
}
