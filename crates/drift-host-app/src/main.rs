//! `Drift Host.app`: the menu-bar app (task **HOST-9**). Starts the host, keeps the menu in step
//! with it once a second, and explains missing permissions at launch. Since HOST-15 it also
//! guides setup: it asks for the Mac's password, shows how to add a device once the host runs,
//! and asks the user to approve a device that wants to pair.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    app::main();
}

#[cfg(target_os = "macos")]
mod app {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use block2::RcBlock;
    use drift_host::launch::{self, Launched};
    use drift_host::pairing::{CODE_LIFETIME, PendingApproval};
    use drift_host_app::model::{
        self, DeviceSetupInfo, HostStatus, MenuAction, MenuInput, NoPairing, PairedDevices, Permission,
        SetupStep, approval_prompt, auto_show, device_setup, first_run_alert, guidance, icon_state,
        password_prompt, setup_step,
    };
    use drift_host_app::setup_window::{SetupAction, SetupWindow};
    use drift_host_app::status_item::{StatusItem, build_menu};
    use drift_host_app::{account, login, permissions};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication,
        NSApplicationActivationPolicy, NSPasteboard, NSPasteboardTypeString,
    };
    use objc2_foundation::{NSString, NSTimer};

    struct App {
        item: StatusItem,
        /// The host's config directory.
        dir: Option<PathBuf>,
        /// The Mac account devices sign in with.
        account: String,
        launched: Option<Launched>,
        error: Option<String>,
        /// No password in the Keychain yet (HOST-15).
        needs_password: bool,
        shown: Option<MenuInput>,
        /// The setup step at the last refresh (HOST-15).
        step: Option<SetupStep>,
        /// The "Set Up a Device" window.
        setup: Option<SetupWindow>,
        /// The approval request on screen.
        asking: Option<u64>,
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

        let dir = drift_host::config::default_dir();
        let account = dir
            .as_deref()
            .and_then(|d| launch::load_config(d, None).ok())
            .map(|c| c.username)
            .unwrap_or_default();
        let mut state = App {
            item: StatusItem::new(mtm),
            dir,
            account,
            launched: None,
            error: None,
            needs_password: false,
            shown: None,
            step: None,
            setup: None,
            asking: None,
        };
        start_host(&mut state);
        APP.with(|a| *a.borrow_mut() = Some(state));
        show_first_run_alert(mtm);
        refresh(mtm);

        let tick = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| refresh(mtm));
        // SAFETY: a repeating timer on the main run loop; the block only touches main-thread state.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &tick) };
        app.run();
    }

    /// (Re)starts the host: stops a running one, then starts from the config directory, or
    /// waits for the password when the Keychain has none.
    fn start_host(app: &mut App) {
        if let Some(l) = app.launched.take() {
            l.host.stop();
        }
        app.error = None;
        app.needs_password = false;
        let Some(dir) = app.dir.clone() else {
            app.error = Some("no config directory (HOME is not set)".to_owned());
            return;
        };
        if !launch::has_password() {
            app.needs_password = true;
            return;
        }
        match launch::start(&dir, None) {
            Ok(l) => {
                tracing::info!(addr = %l.host.addr(), fingerprint = %l.fingerprint, "Drift Host listening");
                app.launched = Some(l);
            }
            Err(e) => app.error = Some(e),
        }
    }

    fn status(app: &App) -> HostStatus {
        if app.needs_password {
            return HostStatus::NeedsPassword;
        }
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

    /// What a refresh found that needs a window (opened after the app state is released).
    enum Next {
        Nothing,
        Show(SetupStep),
        Approve(PendingApproval),
    }

    /// Rebuilds the menu when anything it shows changed, and opens the next setup window or an
    /// approval prompt (main thread).
    fn refresh(mtm: MainThreadMarker) {
        let next = APP.with(|a| {
            let mut guard = a.borrow_mut();
            let Some(app) = guard.as_mut() else { return Next::Nothing };
            let status = status(app);
            let reached = matches!(status, HostStatus::Connected { .. })
                && app.launched.as_ref().is_some_and(|l| l.config.listens_beyond_loopback());
            let pairing = app.launched.as_ref().and_then(|l| l.pairing.as_ref());
            let input = MenuInput {
                permissions: permissions::current(reached),
                devices: match pairing {
                    Some(pairing) => PairedDevices::devices(pairing),
                    // Pairing is on unless the config says otherwise; before the host runs,
                    // assume the default.
                    None if app.needs_password => Some(Vec::new()),
                    None => NoPairing.devices(),
                },
                start_at_login: login::status() == login::LoginItem::On,
                status,
            };
            let step = setup_step(&input.status, &input.permissions, input.devices.as_ref().map(Vec::len));
            let show = auto_show(app.step, step);
            app.step = Some(step);
            let approval = match (pairing, app.asking) {
                (Some(p), None) => p.approvals().into_iter().next(),
                _ => None,
            };
            if app.shown.as_ref() != Some(&input) {
                app.item.set_attention(
                    icon_state(&input.status, &input.permissions) == model::IconState::Attention,
                );
                app.item.set_menu(&build_menu(mtm, &model::menu(&input), move |action| choose(mtm, action)));
                app.shown = Some(input);
            }
            match (approval, show) {
                (Some(request), _) => {
                    app.asking = Some(request.id);
                    Next::Approve(request)
                }
                (None, Some(step)) => Next::Show(step),
                (None, None) => Next::Nothing,
            }
        });
        match next {
            Next::Nothing | Next::Show(SetupStep::Permission(_) | SetupStep::Ready) => {}
            Next::Show(SetupStep::Password) => ask_password(mtm),
            Next::Show(SetupStep::AddDevice) => open_setup(mtm),
            Next::Approve(request) => ask_approval(mtm, &request),
        }
    }

    fn choose(mtm: MainThreadMarker, action: MenuAction) {
        match action {
            MenuAction::OpenSettings(permission) => open_settings(permission),
            MenuAction::SetPassword => ask_password(mtm),
            MenuAction::SetUpDevice => open_setup(mtm),
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

    /// Asks for the Mac's password until it is right (or the user cancels), saves it and
    /// restarts the host with it.
    fn ask_password(mtm: MainThreadMarker) {
        let Some((dir, user)) =
            APP.with(|a| a.borrow().as_ref().and_then(|app| Some((app.dir.clone()?, app.account.clone()))))
        else {
            return;
        };
        let mut error: Option<String> = None;
        loop {
            let Some(password) = account::ask(mtm, &password_prompt(&user, error.as_deref())) else { return };
            let password = zeroize::Zeroizing::new(password);
            if password.is_empty() {
                error = Some("Enter a password.".to_owned());
                continue;
            }
            // A typo is caught here rather than as a failed sign-in on the device.
            if account::verify_password(&user, &password) == Some(false) {
                error = Some("That password is not right.".to_owned());
                continue;
            }
            if let Err(e) = launch::set_password(&dir, &password) {
                error = Some(format!("Could not save the password: {e}"));
                continue;
            }
            break;
        }
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                start_host(app);
            }
        });
        refresh(mtm);
    }

    /// Opens (or reopens with a fresh code) the "Set Up a Device" window.
    fn open_setup(mtm: MainThreadMarker) {
        let built = APP.with(|a| {
            let guard = a.borrow();
            let app = guard.as_ref()?;
            let launched = app.launched.as_ref()?;
            let pairing = launched.pairing.as_ref()?;
            // Every opening shows a fresh code, valid for the whole lifetime.
            let code = pairing.open_window();
            let a = &launched.announcement;
            let info = DeviceSetupInfo {
                mac_name: a.name.clone(),
                host: a.host.clone(),
                port: a.port,
                account: launched.config.username.clone(),
                code: Some(code.to_string()),
                minutes_left: Some(CODE_LIFETIME.as_secs() / 60),
            };
            Some((device_setup(&info), code.to_string(), a.pairing_qr_modules(&code), a.pairing_link(&code)))
        });
        let Some((setup, code, qr, link)) = built else { return };
        let window = SetupWindow::show(mtm, &setup, Some(&code), qr.as_ref(), move |action| match action {
            SetupAction::CopyLink => copy(&link),
            // Rebuilt on the next turn of the run loop: this window's target is still running.
            SetupAction::NewCode => later(mtm, move || open_setup(mtm)),
            SetupAction::Done => APP.with(|a| {
                if let Some(window) = a.borrow().as_ref().and_then(|app| app.setup.as_ref()) {
                    window.close();
                }
            }),
        });
        // Replaced after the borrow: dropping the old window closes it.
        let old = APP.with(|a| a.borrow_mut().as_mut().and_then(|app| app.setup.replace(window)));
        drop(old);
    }

    /// Asks the Mac's user whether `request`'s device may pair, and answers it.
    fn ask_approval(mtm: MainThreadMarker, request: &PendingApproval) {
        let prompt = approval_prompt(&request.device_name);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&prompt.title));
        alert.setInformativeText(&NSString::from_str(&prompt.body));
        // The first button is the default: Return never lets a device in.
        alert.addButtonWithTitle(&NSString::from_str(&prompt.deny));
        alert.addButtonWithTitle(&NSString::from_str(&prompt.allow));
        NSApplication::sharedApplication(mtm).activate();
        let response = alert.runModal();
        let allowed = APP.with(|a| {
            let mut guard = a.borrow_mut();
            let app = guard.as_mut()?;
            app.asking = None;
            let pairing = app.launched.as_ref()?.pairing.as_ref()?;
            Some(match response {
                r if r == NSAlertSecondButtonReturn => pairing.approve(request.id),
                r if r == NSAlertFirstButtonReturn => !pairing.decline(request.id),
                _ => false,
            })
        });
        if allowed == Some(true) {
            // The device is set up; its window has done its job.
            let old = APP.with(|a| a.borrow_mut().as_mut().and_then(|app| app.setup.take()));
            drop(old);
        }
        refresh(mtm);
    }

    fn copy(text: &str) {
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        // SAFETY: an immutable constant exported by AppKit.
        let kind = unsafe { NSPasteboardTypeString };
        board.setString_forType(&NSString::from_str(text), kind);
    }

    /// Runs `f` on the next turn of the main run loop.
    fn later(_mtm: MainThreadMarker, f: impl Fn() + 'static) {
        let block = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| f());
        // SAFETY: a one-shot timer on the main run loop; the block only touches main-thread state.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
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
