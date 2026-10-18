//! The background services behind "reach this Mac after a restart" (task **HOST-13**, ADR
//! `HOST-13-login-window`).
//!
//! | Service | launchd | How it is installed |
//! |---|---|---|
//! | the broker daemon (root) | `LaunchDaemons/` [`DAEMON_PLIST`] | `SMAppService.daemon`, approved once in System Settings › General › Login Items |
//! | the user's agent | `LaunchAgents/` [`AGENT_PLIST`], `Aqua` | `SMAppService.agent`, per user |
//! | the login-window agent | `LaunchAgents/` [`LOGINWINDOW_PLIST`], `LoginWindow` | copied to `/Library/LaunchAgents` by an administrator (`SMAppService` agents load per user, never at the login window) |
//!
//! The pure list is here; [`register`] is the humble `SMAppService` call (needs the signed
//! bundle; tests never call it).

/// The daemon's property list in `Contents/Library/LaunchDaemons`.
pub const DAEMON_PLIST: &str = "com.hutsonlabs.drift.host.daemon.plist";
/// The user agent's property list in `Contents/Library/LaunchAgents`.
pub const AGENT_PLIST: &str = "com.hutsonlabs.drift.host.agent.plist";
/// The login-window agent's property list in `Contents/Library/LaunchAgents`.
pub const LOGINWINDOW_PLIST: &str = "com.hutsonlabs.drift.host.loginwindow.plist";

/// One background service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// The root broker.
    Daemon,
    /// The logged-in user's agent.
    Agent,
    /// The login-window agent.
    LoginWindow,
}

impl Service {
    /// Its property list's file name.
    pub const fn plist(&self) -> &'static str {
        match self {
            Self::Daemon => DAEMON_PLIST,
            Self::Agent => AGENT_PLIST,
            Self::LoginWindow => LOGINWINDOW_PLIST,
        }
    }

    /// Its directory inside `Contents/Library`.
    pub const fn folder(&self) -> &'static str {
        match self {
            Self::Daemon => "LaunchDaemons",
            Self::Agent | Self::LoginWindow => "LaunchAgents",
        }
    }

    /// Whether `SMAppService` can register it (the login-window agent needs an administrator
    /// to copy it to `/Library/LaunchAgents`).
    pub const fn via_service_management(&self) -> bool {
        !matches!(self, Self::LoginWindow)
    }
}

/// Every service, in install order.
pub const fn all() -> [Service; 3] {
    [Service::Daemon, Service::Agent, Service::LoginWindow]
}

/// Registers `service` with `SMAppService` (`daemonServiceWithPlistName:` /
/// `agentServiceWithPlistName:`). The daemon then waits for the user's approval in System
/// Settings. Needs the signed app bundle.
#[cfg(target_os = "macos")]
pub fn register(service: Service) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSError, NSString};

    if !service.via_service_management() {
        return Err(
            "the login-window agent is installed by an administrator into /Library/LaunchAgents".into()
        );
    }
    let class = AnyClass::get(c"SMAppService").ok_or("ServiceManagement is not available")?;
    let name = NSString::from_str(service.plist());
    // SAFETY: both class methods take an NSString and return an SMAppService (or nil).
    let object: Option<Retained<AnyObject>> = unsafe {
        match service {
            Service::Daemon => msg_send![class, daemonServiceWithPlistName: &*name],
            _ => msg_send![class, agentServiceWithPlistName: &*name],
        }
    };
    let object = object.ok_or("no service for this bundle")?;
    let mut error: *mut NSError = std::ptr::null_mut();
    // SAFETY: `registerAndReturnError:` takes an `NSError **` and returns BOOL.
    let ok: bool = unsafe { msg_send![&*object, registerAndReturnError: &mut error] };
    if ok {
        return Ok(());
    }
    // SAFETY: on failure the error is set (autoreleased) or nil.
    Err(unsafe { error.as_ref() }
        .map_or_else(|| "registration failed".into(), |e| e.localizedDescription().to_string()))
}
