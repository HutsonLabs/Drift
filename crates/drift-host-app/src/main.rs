//! `Drift Host.app`: the menu-bar app (task **HOST-9**). Starts the host, keeps the menu in step
//! with it once a second, and explains missing permissions at launch.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    app::main();
}

#[cfg(target_os = "macos")]
mod app {
    use std::cell::RefCell;

    use block2::RcBlock;
    use drift_host::launch::{self, Launched};
    use drift_host_app::model::{
        self, HostStatus, MenuAction, MenuInput, NoPairing, PairedDevices, Permission, first_run_alert,
        guidance, icon_state,
    };
    use drift_host_app::status_item::{StatusItem, build_menu};
    use drift_host_app::{login, permissions};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{NSString, NSTimer};

    struct App {
        item: StatusItem,
        launched: Option<Launched>,
        error: Option<String>,
        shown: Option<MenuInput>,
    }

    thread_local! {
        static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    }

    pub fn main() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let (launched, error) = match drift_host::config::default_dir()
            .ok_or_else(|| "no config directory (HOME is not set)".to_owned())
            .and_then(|dir| launch::start(&dir, None))
        {
            Ok(l) => {
                tracing::info!(addr = %l.host.addr(), fingerprint = %l.fingerprint, "Drift Host listening");
                (Some(l), None)
            }
            Err(e) => (None, Some(e)),
        };
        APP.with(|a| {
            *a.borrow_mut() = Some(App { item: StatusItem::new(mtm), launched, error, shown: None })
        });
        refresh(mtm);
        show_first_run_alert(mtm);

        let tick = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| refresh(mtm));
        // SAFETY: a repeating timer on the main run loop; the block only touches main-thread state.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &tick) };
        app.run();
    }

    fn status(app: &App) -> HostStatus {
        match (&app.launched, &app.error) {
            (_, Some(error)) => HostStatus::Stopped { error: error.clone() },
            (Some(l), None) => {
                let stats = l.host.stats();
                if stats.sessions_served > stats.sessions_ended {
                    HostStatus::Connected { device: None }
                } else {
                    HostStatus::Listening {
                        addr: l.host.addr().to_string(),
                        loopback_only: !l.config.listens_beyond_loopback(),
                    }
                }
            }
            (None, None) => HostStatus::Starting,
        }
    }

    /// Rebuilds the menu when anything it shows changed (main thread).
    fn refresh(mtm: MainThreadMarker) {
        APP.with(|a| {
            let mut guard = a.borrow_mut();
            let Some(app) = guard.as_mut() else { return };
            let status = status(app);
            let reached = matches!(status, HostStatus::Connected { .. })
                && app.launched.as_ref().is_some_and(|l| l.config.listens_beyond_loopback());
            let input = MenuInput {
                permissions: permissions::current(reached),
                devices: match app.launched.as_ref().and_then(|l| l.pairing.as_ref()) {
                    Some(pairing) => PairedDevices::devices(pairing),
                    None => NoPairing.devices(),
                },
                start_at_login: login::status() == login::LoginItem::On,
                status,
            };
            if app.shown.as_ref() == Some(&input) {
                return;
            }
            app.item
                .set_attention(icon_state(&input.status, &input.permissions) == model::IconState::Attention);
            app.item.set_menu(&build_menu(mtm, &model::menu(&input), move |action| choose(mtm, action)));
            app.shown = Some(input);
        });
    }

    fn choose(mtm: MainThreadMarker, action: MenuAction) {
        match action {
            MenuAction::OpenSettings(permission) => open_settings(permission),
            MenuAction::ToggleStartAtLogin => {
                let on = login::status() == login::LoginItem::On;
                if let Err(e) = login::set(!on) {
                    tracing::warn!(error = %e, "could not change Start at Login");
                }
                refresh(mtm);
            }
            MenuAction::Quit => {
                APP.with(|a| {
                    if let Some(app) = a.borrow_mut().as_mut()
                        && let Some(l) = app.launched.take()
                    {
                        l.host.stop();
                    }
                });
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }

    fn open_settings(permission: Permission) {
        match permission {
            Permission::ScreenRecording => {
                permissions::request_screen_recording();
            }
            Permission::Accessibility => {
                permissions::request_accessibility();
            }
            Permission::LocalNetwork => {}
        }
        permissions::open_settings(&guidance(permission).settings_url);
    }

    fn show_first_run_alert(mtm: MainThreadMarker) {
        let Some(alert) = first_run_alert(&permissions::current(false)) else { return };
        let ns = NSAlert::new(mtm);
        ns.setMessageText(&NSString::from_str(&alert.title));
        ns.setInformativeText(&NSString::from_str(&alert.body));
        ns.addButtonWithTitle(&NSString::from_str("Open Settings"));
        ns.addButtonWithTitle(&NSString::from_str("Later"));
        NSApplication::sharedApplication(mtm).activate();
        if ns.runModal() == NSAlertFirstButtonReturn {
            open_settings(alert.open);
        }
    }
}
