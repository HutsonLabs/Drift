//! The menu-bar app's view model (task **HOST-9**): what the status line says, what the menu
//! holds, when the icon asks for attention, and the plain-language guidance for each permission.
//!
//! Pure: the AppKit side ([`crate::status_item`]) only turns [`MenuEntry`]s into `NSMenuItem`s
//! and sends [`MenuAction`]s back.

/// What the host is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    /// The listener is being set up.
    Starting,
    /// Waiting for a client.
    Listening {
        /// `address:port` the listener is bound to.
        addr: String,
        /// Bound to loopback only (the default until pairing exists, ADR `HOST-2-host-skeleton`).
        loopback_only: bool,
    },
    /// A client is connected.
    Connected {
        /// The device's name, once pairing (HOST-8) can tell.
        device: Option<String>,
    },
    /// The host could not start or stopped with an error.
    Stopped {
        /// What went wrong, in words.
        error: String,
    },
}

/// A permission Drift Host needs from macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Screen Recording: to capture the screen (HOST-3).
    ScreenRecording,
    /// Accessibility: to type and move the pointer for the client (HOST-6).
    Accessibility,
    /// Local Network: to accept connections from devices on the network.
    LocalNetwork,
}

/// Whether a permission is granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Granted.
    Granted,
    /// Not granted (yet).
    Missing,
    /// macOS offers no way to ask in advance (Local Network).
    Unknown,
}

/// The state of every permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// Screen Recording.
    pub screen_recording: Grant,
    /// Accessibility.
    pub accessibility: Grant,
    /// Local Network.
    pub local_network: Grant,
}

impl Permissions {
    /// The grant of `permission`.
    pub fn get(&self, permission: Permission) -> Grant {
        match permission {
            Permission::ScreenRecording => self.screen_recording,
            Permission::Accessibility => self.accessibility,
            Permission::LocalNetwork => self.local_network,
        }
    }
}

/// A device paired with this host (filled by HOST-8's pairing store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// The device's name.
    pub name: String,
    /// When it last connected, in words ("today", "Oct 12").
    pub last_seen: Option<String>,
}

/// Where the menu gets the paired devices from. `None` means this build has no pairing yet.
pub trait PairedDevices {
    /// The paired devices, or `None` without pairing support.
    fn devices(&self) -> Option<Vec<PairedDevice>>;
}

/// No pairing support (until HOST-8).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPairing;

impl PairedDevices for NoPairing {
    fn devices(&self) -> Option<Vec<PairedDevice>> {
        None
    }
}

/// What a menu item does when chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Show the guidance for a permission and open its Settings pane.
    OpenSettings(Permission),
    /// Register or unregister the app as a login item.
    ToggleStartAtLogin,
    /// Quit Drift Host.
    Quit,
}

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A line of information (disabled).
    Text(String),
    /// A choosable item.
    Item {
        /// Its title.
        title: String,
        /// What choosing it does.
        action: MenuAction,
        /// Shows a check mark.
        checked: bool,
    },
    /// A submenu.
    Submenu {
        /// Its title.
        title: String,
        /// Its entries.
        entries: Vec<MenuEntry>,
    },
    /// A separator line.
    Separator,
}

/// Everything the menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuInput {
    /// The host's status.
    pub status: HostStatus,
    /// The permissions.
    pub permissions: Permissions,
    /// Paired devices, `None` without pairing support.
    pub devices: Option<Vec<PairedDevice>>,
    /// Registered as a login item.
    pub start_at_login: bool,
}

/// The menu-bar icon's look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Waiting for a client.
    Idle,
    /// A client is connected.
    Connected,
    /// Something needs the user: a missing permission or a stopped host.
    Attention,
}

/// The first line of the menu.
pub fn status_line(status: &HostStatus) -> String {
    match status {
        HostStatus::Starting => "Starting…".to_owned(),
        HostStatus::Listening { addr, loopback_only: true } => format!("Waiting on {addr} (this Mac only)"),
        HostStatus::Listening { addr, loopback_only: false } => {
            let port = addr.rsplit(':').next().unwrap_or(addr);
            format!("Waiting for a device on port {port}")
        }
        HostStatus::Connected { device: Some(name) } => format!("Connected: {name}"),
        HostStatus::Connected { device: None } => "A device is connected".to_owned(),
        HostStatus::Stopped { error } => format!("Stopped: {error}"),
    }
}

/// The icon's look: attention while the host is stopped or capture/input permissions are
/// missing (Local Network cannot be checked in advance, so unknown is no alarm).
pub fn icon_state(status: &HostStatus, permissions: &Permissions) -> IconState {
    let missing = [permissions.screen_recording, permissions.accessibility, permissions.local_network]
        .contains(&Grant::Missing);
    match status {
        HostStatus::Stopped { .. } => IconState::Attention,
        _ if missing => IconState::Attention,
        HostStatus::Connected { .. } => IconState::Connected,
        HostStatus::Starting | HostStatus::Listening { .. } => IconState::Idle,
    }
}

const ORDER: [Permission; 3] =
    [Permission::ScreenRecording, Permission::Accessibility, Permission::LocalNetwork];

/// The menu, top to bottom: status, paired devices, missing permissions, Start at Login, Quit.
pub fn menu(input: &MenuInput) -> Vec<MenuEntry> {
    let mut entries = vec![MenuEntry::Text(status_line(&input.status)), MenuEntry::Separator];
    entries.push(match &input.devices {
        None => MenuEntry::Text("Paired devices: none yet (pairing comes in a later version)".to_owned()),
        Some(devices) if devices.is_empty() => MenuEntry::Text("No paired devices".to_owned()),
        Some(devices) => MenuEntry::Submenu {
            title: "Paired Devices".to_owned(),
            entries: devices
                .iter()
                .map(|d| {
                    MenuEntry::Text(match &d.last_seen {
                        Some(when) => format!("{} — last seen {when}", d.name),
                        None => d.name.clone(),
                    })
                })
                .collect(),
        },
    });
    let missing: Vec<Permission> =
        ORDER.into_iter().filter(|p| input.permissions.get(*p) == Grant::Missing).collect();
    if !missing.is_empty() {
        entries.push(MenuEntry::Separator);
        for p in missing {
            entries.push(MenuEntry::Item {
                title: format!("{}…", guidance(p).title),
                action: MenuAction::OpenSettings(p),
                checked: false,
            });
        }
    }
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Item {
        title: "Start at Login".to_owned(),
        action: MenuAction::ToggleStartAtLogin,
        checked: input.start_at_login,
    });
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Item {
        title: "Quit Drift Host".to_owned(),
        action: MenuAction::Quit,
        checked: false,
    });
    entries
}

/// What to tell the user about one permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guidance {
    /// "Allow Screen Recording".
    pub title: String,
    /// Why Drift Host needs it, in one sentence.
    pub why: String,
    /// What to do, one short step per line.
    pub steps: Vec<String>,
    /// The System Settings pane (`x-apple.systempreferences:` URL).
    pub settings_url: String,
}

const PRIVACY: &str = "x-apple.systempreferences:com.apple.preference.security?";

/// The guidance for `permission`.
pub fn guidance(permission: Permission) -> Guidance {
    let (title, why, steps, pane): (&str, &str, &[&str], &str) = match permission {
        Permission::ScreenRecording => (
            "Allow Screen Recording",
            "Drift Host needs it to see your screen and send it to your device.",
            &[
                "Open System Settings › Privacy & Security › Screen & System Audio Recording.",
                "Turn on Drift Host.",
                "Quit and reopen Drift Host when macOS asks.",
            ],
            "Privacy_ScreenCapture",
        ),
        Permission::Accessibility => (
            "Allow Accessibility",
            "Drift Host needs it to use the keyboard and mouse for you while a device is connected.",
            &[
                "Open System Settings › Privacy & Security › Accessibility.",
                "Turn on Drift Host. Use your password or Touch ID if asked.",
            ],
            "Privacy_Accessibility",
        ),
        Permission::LocalNetwork => (
            "Allow Local Network",
            "Drift Host needs it so iPhones, iPads and other Macs on your network can reach it.",
            &[
                "When macOS asks whether Drift Host may find devices on your network, choose Allow.",
                "If you chose Don't Allow: open System Settings › Privacy & Security › Local Network.",
                "Turn on Drift Host.",
            ],
            "Privacy_LocalNetwork",
        ),
    };
    Guidance {
        title: title.to_owned(),
        why: why.to_owned(),
        steps: steps.iter().map(|s| (*s).to_owned()).collect(),
        settings_url: format!("{PRIVACY}{pane}"),
    }
}

/// First-run guidance: every permission that is missing (or, for Local Network, cannot be
/// checked yet), in the order the user should grant them.
pub fn first_run(permissions: &Permissions) -> Vec<Guidance> {
    ORDER.into_iter().filter(|p| permissions.get(*p) != Grant::Granted).map(guidance).collect()
}

/// The alert shown at launch while capture or input permissions are missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstRunAlert {
    /// "Drift Host needs 2 permissions" / "Drift Host needs Accessibility".
    pub title: String,
    /// Why and what to do for the first missing permission, numbered steps.
    pub body: String,
    /// The permission whose Settings pane "Open Settings" opens.
    pub open: Permission,
}

/// The launch alert, or `None` when Screen Recording and Accessibility are both granted.
/// Local Network is left to macOS's own prompt, which appears when a device first connects.
pub fn first_run_alert(permissions: &Permissions) -> Option<FirstRunAlert> {
    let missing: Vec<Permission> = [Permission::ScreenRecording, Permission::Accessibility]
        .into_iter()
        .filter(|p| permissions.get(*p) != Grant::Granted)
        .collect();
    let first = *missing.first()?;
    let g = guidance(first);
    let title = match missing.len() {
        1 => format!("Drift Host needs {}", g.title.trim_start_matches("Allow ")),
        n => format!("Drift Host needs {n} permissions"),
    };
    let steps: Vec<String> = g.steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect();
    Some(FirstRunAlert { title, body: format!("{}\n\n{}", g.why, steps.join("\n")), open: first })
}
