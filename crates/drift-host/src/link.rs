//! Link hygiene (task **HOST-7**, ADR `HOST-7-link-hygiene`).
//!
//! * The session socket carries the interactive-video service class (`SO_NET_SERVICE_TYPE` =
//!   `NET_SERVICE_TYPE_VI`), which puts its packets in Wi-Fi's video access category (WMM AC_VI).
//! * [`IdleHeartbeat`] says when an idle link should carry something small, so a phone's Wi-Fi
//!   radio does not drop into power save between pictures.
//! * [`BacklogMeter`] turns frames awaiting acknowledgement and bytes waiting in the socket into a
//!   [`Backlog`] level with hysteresis, for EXT-4's quality hints.
//!
//! The socket calls are humble (`setsockopt`/`getsockopt`); the rest is pure.

use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::time::{Duration, Instant};

/// `SO_NET_SERVICE_TYPE` (`<sys/socket.h>`).
pub const SO_NET_SERVICE_TYPE: libc::c_int = 0x1116;

/// `SO_NWRITE`: bytes in the socket's send buffer not yet acknowledged by the peer.
pub const SO_NWRITE: libc::c_int = 0x1024;

/// `NET_SERVICE_TYPE_VI`: interactive video.
pub const NET_SERVICE_TYPE_VI: libc::c_int = 3;

fn set_int(fd: RawFd, option: libc::c_int, value: libc::c_int) -> io::Result<()> {
    let len = libc::socklen_t::try_from(size_of::<libc::c_int>()).unwrap_or(4);
    // SAFETY: a valid pointer to an int of the given length for the duration of the call.
    let rc = unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, option, (&raw const value).cast(), len) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn get_int(fd: RawFd, option: libc::c_int) -> io::Result<libc::c_int> {
    let mut value: libc::c_int = 0;
    let mut len = libc::socklen_t::try_from(size_of::<libc::c_int>()).unwrap_or(4);
    // SAFETY: valid out-pointers for an int and its length for the duration of the call.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, option, (&raw mut value).cast(), &raw mut len) };
    if rc == 0 { Ok(value) } else { Err(io::Error::last_os_error()) }
}

/// Sets the socket's network service type.
pub fn set_service_class(fd: RawFd, class: libc::c_int) -> io::Result<()> {
    set_int(fd, SO_NET_SERVICE_TYPE, class)
}

/// The socket's network service type.
pub fn service_class(fd: RawFd) -> io::Result<libc::c_int> {
    get_int(fd, SO_NET_SERVICE_TYPE)
}

/// Reads a session socket's send-buffer occupancy through a duplicate descriptor, so it never
/// touches a descriptor number the connection has closed and the system reused.
#[derive(Debug)]
pub struct SocketProbe {
    fd: OwnedFd,
}

impl SocketProbe {
    /// Duplicates `fd`.
    pub fn new(fd: RawFd) -> io::Result<Self> {
        // SAFETY: `dup` on a descriptor the caller owns; a non-negative result is ours.
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `dup` returned a fresh descriptor nobody else owns.
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(dup) } })
    }

    /// Bytes written but not yet acknowledged by the peer.
    pub fn unsent_bytes(&self) -> io::Result<u64> {
        get_int(self.fd.as_raw_fd(), SO_NWRITE).map(|n| u64::try_from(n).unwrap_or(0))
    }
}

/// The idle interval after which the host sends a heartbeat. Phone Wi-Fi radios enter power save
/// after a few hundred milliseconds without traffic; 100 ms keeps them awake at ~10 tiny
/// updates a second, and only while nothing else is sent.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);

/// When an idle link is due a heartbeat.
#[derive(Debug, Clone, Copy)]
pub struct IdleHeartbeat {
    interval: Duration,
    last_out: Option<Instant>,
}

impl IdleHeartbeat {
    /// A heartbeat after `interval` without output.
    pub const fn new(interval: Duration) -> Self {
        Self { interval, last_out: None }
    }

    /// Something (a picture, a pointer update, a heartbeat) went out at `at`.
    pub fn sent(&mut self, at: Instant) {
        self.last_out = Some(at);
    }

    /// Whether a heartbeat is due at `now`.
    pub fn due(&self, now: Instant) -> bool {
        self.last_out.is_none_or(|t| now.saturating_duration_since(t) >= self.interval)
    }
}

/// How far behind the client is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Backlog {
    /// Keeping up.
    #[default]
    Clear,
    /// Falling behind: frames and bytes are piling up.
    Building,
    /// Well behind: lower the quality (EXT-4).
    Backlogged,
}

/// One look at the link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BacklogSample {
    /// GFX frames sent and not yet acknowledged.
    pub frames_in_flight: u32,
    /// Bytes in the socket's send buffer.
    pub unsent_bytes: u64,
}

impl BacklogSample {
    /// The level this sample alone points at.
    pub const fn level(self) -> Backlog {
        if self.frames_in_flight >= BACKLOGGED_FRAMES || self.unsent_bytes >= BACKLOGGED_BYTES {
            Backlog::Backlogged
        } else if self.frames_in_flight >= BUILDING_FRAMES || self.unsent_bytes >= BUILDING_BYTES {
            Backlog::Building
        } else {
            Backlog::Clear
        }
    }
}

/// Frames in flight that mean the backlog is building.
pub const BUILDING_FRAMES: u32 = 3;
/// … and that mean it is backlogged.
pub const BACKLOGGED_FRAMES: u32 = 6;
/// Unsent bytes that mean the backlog is building (about 50 ms at 10 Mbit/s).
pub const BUILDING_BYTES: u64 = 64 * 1024;
/// … and that mean it is backlogged.
pub const BACKLOGGED_BYTES: u64 = 512 * 1024;
/// Samples in a row that must agree before the level rises.
pub const RISE_SAMPLES: u32 = 3;
/// Quiet samples in a row before the level falls.
pub const FALL_SAMPLES: u32 = 5;

/// A [`Backlog`] level with hysteresis: it rises after [`RISE_SAMPLES`] samples at a higher
/// level and falls after [`FALL_SAMPLES`] samples at a lower one.
#[derive(Debug, Clone, Default)]
pub struct BacklogMeter {
    level: Backlog,
    higher: u32,
    lower: u32,
    transitions: u32,
}

impl BacklogMeter {
    /// Folds in one sample and returns the level.
    pub fn update(&mut self, sample: BacklogSample) -> Backlog {
        let seen = sample.level();
        if seen > self.level {
            self.lower = 0;
            self.higher += 1;
            if self.higher >= RISE_SAMPLES {
                self.set(seen);
            }
        } else if seen < self.level {
            self.higher = 0;
            self.lower += 1;
            if self.lower >= FALL_SAMPLES {
                self.set(seen);
            }
        } else {
            self.higher = 0;
            self.lower = 0;
        }
        self.level
    }

    fn set(&mut self, level: Backlog) {
        self.level = level;
        self.higher = 0;
        self.lower = 0;
        self.transitions += 1;
    }

    /// The current level.
    pub const fn level(&self) -> Backlog {
        self.level
    }

    /// Level changes so far.
    pub const fn transitions(&self) -> u32 {
        self.transitions
    }
}

impl PartialOrd for Backlog {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Backlog {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |b: &Self| match b {
            Self::Clear => 0,
            Self::Building => 1,
            Self::Backlogged => 2,
        };
        rank(self).cmp(&rank(other))
    }
}
