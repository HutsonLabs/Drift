//! Which path a connection takes (task **HOST-14**, ADR `HOST-14-extra-paths`).
//!
//! Drift reaches a Mac over the LAN, over peer-to-peer Wi-Fi (AWDL, `awdl0`/`llw0`, found with
//! Bonjour when there is no shared network), or over the user's own VPN (`utun*`, `ipsec*`,
//! `ppp*`). There is no NAT traversal: anything else is "remote" and needs that VPN. [`classify`]
//! is pure; [`interfaces`] reads the machine's addresses (`getifaddrs`).

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One address of one interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// BSD name (`en0`, `awdl0`, `utun4`).
    pub name: String,
    /// The address.
    pub addr: IpAddr,
    /// Its prefix length.
    pub prefix: u8,
}

/// How a host is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPath {
    /// This machine.
    Local,
    /// The local network.
    Lan {
        /// The interface.
        interface: String,
    },
    /// Peer-to-peer Wi-Fi (AWDL).
    PeerToPeer {
        /// The interface.
        interface: String,
    },
    /// The user's VPN.
    Vpn {
        /// The tunnel interface.
        interface: String,
    },
    /// None of the above: through a router this machine knows nothing about.
    Remote,
}

impl NetworkPath {
    /// Away from the local network (EXT-4 picks a separate default quality).
    pub fn is_away(&self) -> bool {
        matches!(self, Self::Vpn { .. } | Self::Remote)
    }
}

fn is_peer_to_peer(name: &str) -> bool {
    name.starts_with("awdl") || name.starts_with("llw")
}

fn is_tunnel(name: &str) -> bool {
    ["utun", "ipsec", "ppp", "tun", "tap"].iter().any(|p| name.starts_with(p))
}

fn path_for(name: &str) -> NetworkPath {
    if is_peer_to_peer(name) {
        NetworkPath::PeerToPeer { interface: name.to_owned() }
    } else if is_tunnel(name) {
        NetworkPath::Vpn { interface: name.to_owned() }
    } else {
        NetworkPath::Lan { interface: name.to_owned() }
    }
}

fn same_prefix(a: IpAddr, b: IpAddr, prefix: u8) -> bool {
    match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let bits = u32::from(prefix.min(32));
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let bits = u32::from(prefix.min(128));
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// The path to `dest` (with its IPv6 scope interface, if any) given this machine's interfaces.
pub fn classify(dest: IpAddr, scope: Option<&str>, interfaces: &[Interface]) -> NetworkPath {
    if dest.is_loopback() {
        return NetworkPath::Local;
    }
    if is_link_local(dest) {
        // A link-local address is only meaningful with its scope.
        return scope.map_or(NetworkPath::Remote, path_for);
    }
    // The longest matching prefix wins, as in routing.
    interfaces
        .iter()
        .filter(|i| !i.addr.is_loopback() && same_prefix(dest, i.addr, i.prefix))
        .max_by_key(|i| i.prefix)
        .map_or(NetworkPath::Remote, |i| path_for(&i.name))
}

fn prefix_of(mask: IpAddr) -> u8 {
    let ones = match mask {
        IpAddr::V4(m) => u32::from(m).count_ones(),
        IpAddr::V6(m) => u128::from(m).count_ones(),
    };
    u8::try_from(ones).unwrap_or(0)
}

/// # Safety
/// `sa` must be null or point to a valid `sockaddr` of its family's size.
unsafe fn ip_of(sa: *const libc::sockaddr) -> Option<IpAddr> {
    // SAFETY: the caller guarantees `sa` is null or a valid sockaddr.
    let family = i32::from(unsafe { sa.as_ref() }?.sa_family);
    match family {
        libc::AF_INET => {
            // SAFETY: an AF_INET sockaddr is a sockaddr_in.
            let sin = unsafe { &*sa.cast::<libc::sockaddr_in>() };
            Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))))
        }
        libc::AF_INET6 => {
            // SAFETY: an AF_INET6 sockaddr is a sockaddr_in6.
            let sin6 = unsafe { &*sa.cast::<libc::sockaddr_in6>() };
            Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
        }
        _ => None,
    }
}

/// This machine's IPv4 and IPv6 addresses (`getifaddrs`).
pub fn interfaces() -> io::Result<Vec<Interface>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates the list into `head`; freed below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    let mut cur = head;
    // SAFETY: walking the list getifaddrs returned; every node is valid until freeifaddrs.
    while let Some(node) = unsafe { cur.as_ref() } {
        // SAFETY: as above; ifa_addr/ifa_netmask are null or valid sockaddrs.
        if let (Some(addr), Some(mask)) = unsafe { (ip_of(node.ifa_addr), ip_of(node.ifa_netmask)) } {
            // SAFETY: ifa_name is a NUL-terminated string owned by the list.
            let name = unsafe { std::ffi::CStr::from_ptr(node.ifa_name) }.to_string_lossy().into_owned();
            out.push(Interface { name, addr, prefix: prefix_of(mask) });
        }
        cur = node.ifa_next;
    }
    // SAFETY: freeing the list getifaddrs allocated.
    unsafe { libc::freeifaddrs(head) };
    Ok(out)
}
