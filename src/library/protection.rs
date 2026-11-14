// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether this computer reaches the internet through a VPN, for listeners
//! who want to share only through one (`library-only-protected`).
//!
//! The kernel is asked which interface a packet to the internet would leave
//! through, for IPv4 and for IPv6: a UDP socket is "connected" to a public
//! address, which sends nothing but picks the route, policy routing such as
//! WireGuard's `wg-quick` sets up included. The connection counts as
//! protected when every reachable address family uses a tunnel. An IPv4-only
//! VPN is also usable: the node must then disable IPv6 for all peer traffic.
//!
//! Tunnels are recognized by what the kernel says they are: tun and tap
//! devices (OpenVPN, Tailscale and most VPN apps), WireGuard, and other
//! interfaces without a link layer. Tor can't be recognized this way, and
//! BitTorrent can't go through it anyway: Tor carries no UDP.

use std::net::{SocketAddr, UdpSocket};

#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Every route to the internet goes through these tunnels
    Protected(Vec<String>),
    /// The only reachable family is IPv4, and it uses a tunnel.
    Ipv4Only(Vec<String>),
    /// A tunnel is active, but another reachable address family bypasses it.
    Partial {
        tunnels: Vec<String>,
        exposed_families: Vec<&'static str>,
    },
    /// Some route doesn't, through this interface if it is known
    Exposed(Option<String>),
    /// There is no route to the internet
    Offline,
}

impl Protection {
    #[cfg(test)]
    pub fn is_protected(&self) -> bool {
        matches!(self, Protection::Protected(_) | Protection::Ipv4Only(_))
    }

    /// Allowed socket mode for VPN-only sharing. `Some(true)` must be
    /// enforced by the node, not merely used to turn the indicator green.
    pub fn sharing_ipv4_only(&self) -> Option<bool> {
        match self {
            Self::Protected(_) => Some(false),
            Self::Ipv4Only(_) => Some(true),
            Self::Partial {
                exposed_families, ..
            } if exposed_families.as_slice() == ["IPv6"] => Some(true),
            _ => None,
        }
    }
}

pub fn check() -> Protection {
    #[cfg(target_os = "android")]
    {
        return match crate::android::vpn() {
            3 => Protection::Protected(vec!["Android VPN".into()]),
            1 => Protection::Partial {
                tunnels: vec!["Android VPN".into()],
                exposed_families: vec!["IPv6"],
            },
            2 => Protection::Partial {
                tunnels: vec!["Android VPN".into()],
                exposed_families: vec!["IPv4"],
            },
            -1 => Protection::Offline,
            _ => Protection::Exposed(None),
        };
    }
    if let Some(system) = torified_system() {
        return Protection::Protected(vec![system.to_string()]);
    }
    let routes = [
        ("IPv4", "1.1.1.1:53"),
        ("IPv6", "[2606:4700:4700::1111]:53"),
    ]
    .iter()
    .filter_map(|(family, target)| {
        #[cfg(target_os = "windows")]
        {
            return windows::route(target).map(|(interface, tunnel)| Route {
                family,
                interface: Some(interface),
                tunnel,
            });
        }
        #[cfg(not(target_os = "windows"))]
        route_interface(target).map(|interface| Route {
            family,
            tunnel: interface.as_deref().is_some_and(is_tunnel),
            interface,
        })
    })
    .collect::<Vec<_>>();
    classify_routes(&routes)
}

struct Route {
    family: &'static str,
    interface: Option<String>,
    tunnel: bool,
}

fn classify_routes(routes: &[Route]) -> Protection {
    if routes.is_empty() {
        return Protection::Offline;
    }
    let mut tunnels = Vec::new();
    let mut exposed_families = Vec::new();
    let mut exposed_interface = None;
    for route in routes {
        if route.tunnel {
            if let Some(name) = &route.interface {
                if !tunnels.contains(name) {
                    tunnels.push(name.clone());
                }
            }
        } else {
            exposed_families.push(route.family);
            if exposed_interface.is_none() {
                exposed_interface = route.interface.clone();
            }
        }
    }
    if tunnels.is_empty() {
        Protection::Exposed(exposed_interface)
    } else if exposed_families.is_empty() && routes.iter().all(|r| r.family == "IPv4") {
        Protection::Ipv4Only(tunnels)
    } else if exposed_families.is_empty() {
        Protection::Protected(tunnels)
    } else {
        Protection::Partial {
            tunnels,
            exposed_families,
        }
    }
}

/// The interface the internet is reached through for `target`'s address
/// family: `None` without a route, `Some(None)` if the interface is unknown.
fn route_interface(target: &str) -> Option<Option<String>> {
    let target: SocketAddr = target.parse().ok()?;
    let any: SocketAddr = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    }
    .parse()
    .ok()?;
    let socket = UdpSocket::bind(any).ok()?;
    socket.connect(target).ok()?;
    let local = socket.local_addr().ok()?.ip();
    Some(interface_with(local))
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "macos"
))]
fn interface_with(ip: std::net::IpAddr) -> Option<String> {
    use std::net::IpAddr;

    nix::ifaddrs::getifaddrs().ok()?.find_map(|ifa| {
        let address = ifa.address?;
        let matches = match ip {
            IpAddr::V4(v4) => address.as_sockaddr_in().is_some_and(|a| a.ip() == v4),
            IpAddr::V6(v6) => address.as_sockaddr_in6().is_some_and(|a| a.ip() == v6),
        };
        matches.then_some(ifa.interface_name)
    })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "macos"
)))]
fn interface_with(_ip: std::net::IpAddr) -> Option<String> {
    None
}

/// Whether an interface is a tunnel, by what the kernel says it is.
#[cfg(target_os = "linux")]
fn is_tunnel(name: &str) -> bool {
    use std::fs;

    if name.contains('/') || name.starts_with('.') {
        return false;
    }
    let dir = std::path::Path::new("/sys/class/net").join(name);
    // tun and tap devices
    if dir.join("tun_flags").exists() {
        return true;
    }
    if fs::read_to_string(dir.join("uevent"))
        .is_ok_and(|u| u.lines().any(|l| l == "DEVTYPE=wireguard"))
    {
        return true;
    }
    // No link layer at all (ARPHRD_NONE), as tun, WireGuard and IPsec
    // interfaces. PPP is left out: it is how many homes reach the
    // internet, not only VPNs
    fs::read_to_string(dir.join("type")).is_ok_and(|t| t.trim() == "65534")
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "macos"
))]
fn is_tunnel(name: &str) -> bool {
    kernel_tunnel_name(name)
}

#[cfg(any(
    test,
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "macos"
))]
fn kernel_tunnel_name(name: &str) -> bool {
    // These are kernel interface names, not user-supplied connection labels.
    // Do not mistake gif/stf IPv6 transition interfaces or generic PPP for VPNs.
    ["utun", "tun", "tap", "wg"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|unit| !unit.is_empty() && unit.bytes().all(|c| c.is_ascii_digit()))
    })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "macos"
)))]
fn is_tunnel(_name: &str) -> bool {
    false
}

/// Everything known about how this computer hides its address, to explain
/// the verdict: what decides is [`check`].
#[derive(Clone, Debug)]
pub struct Status {
    pub protection: Protection,
    /// Network hiding tools found running or listening, by name
    pub tools: Vec<&'static str>,
    /// Programs are asked to use a proxy, which this app's traffic doesn't
    pub system_proxy: bool,
}

pub fn status() -> Status {
    let mut tools: Vec<&'static str> = running_tools();
    for tool in listening_tools() {
        if !tools.contains(&tool) {
            tools.push(tool);
        }
    }
    Status {
        protection: check(),
        tools,
        system_proxy: system_proxy(),
    }
}

/// Systems that send all of their traffic through Tor themselves.
fn torified_system() -> Option<&'static str> {
    let exists = |path: &str| std::path::Path::new(path).exists();
    if exists("/usr/share/anon-ws-base-files/workstation") || exists("/etc/whonix_version") {
        Some("Whonix")
    } else if exists("/etc/amnesia/version") {
        Some("Tails")
    } else {
        None
    }
}

/// Programs that hide where traffic comes from, by the name of their process
/// (as `/proc/<pid>/comm` has it, cut at 15 characters). Inside a Flatpak
/// sandbox no other processes are seen; ports still are.
const PROCESSES: &[(&str, &str)] = &[
    (".clash-verge-se", "Clash"),
    (".clash-verge-wr", "Clash"),
    (".verge-mihomo-w", "Clash"),
    ("clash", "Clash"),
    ("mihomo", "Clash"),
    ("verge-mihomo", "Clash"),
    ("cfw", "Clash"),
    ("sing-box", "sing-box"),
    ("xray", "Xray"),
    ("v2ray", "V2Ray"),
    ("v2raya", "V2Ray"),
    ("ss-local", "Shadowsocks"),
    ("sslocal", "Shadowsocks"),
    ("ss-redir", "Shadowsocks"),
    ("trojan", "Trojan"),
    ("trojan-go", "Trojan"),
    ("hysteria", "Hysteria"),
    ("tuic-client", "TUIC"),
    ("naive", "NaiveProxy"),
    ("tor", "Tor"),
    ("obfs4proxy", "Tor"),
    ("lyrebird", "Tor"),
    ("snowflake-clien", "Tor"),
    ("i2pd", "I2P"),
    ("i2prouter", "I2P"),
    ("psiphon-tunnel-", "Psiphon"),
    ("lantern", "Lantern"),
    ("warp-svc", "Cloudflare WARP"),
    ("nekoray", "NekoBox"),
    ("nekobox", "NekoBox"),
    ("nekobox_core", "NekoBox"),
    ("hiddify", "Hiddify"),
    ("hiddifycli", "Hiddify"),
    ("amneziavpn", "AmneziaVPN"),
    ("amneziavpn-serv", "AmneziaVPN"),
    ("outlineproxycon", "Outline"),
    ("openvpn", "OpenVPN"),
    ("openconnect", "OpenConnect"),
    ("wireguard-go", "WireGuard"),
    ("tailscaled", "Tailscale"),
    ("mullvad-daemon", "Mullvad"),
    ("nordvpnd", "NordVPN"),
    ("expressvpnd", "ExpressVPN"),
    ("protonvpn", "Proton VPN"),
    ("pia-daemon", "PIA"),
    ("windscribe", "Windscribe"),
    ("tun2socks", "tun2socks"),
];

fn running_tools() -> Vec<&'static str> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let comm = comm.trim().to_lowercase();
        if let Some((_, tool)) = PROCESSES.iter().find(|(process, _)| comm == *process) {
            if !found.contains(tool) {
                found.push(tool);
            }
        }
    }
    found
}

/// The usual local ports of such programs, for when processes can't be seen.
const PORTS: &[(u16, &str)] = &[
    (9050, "Tor"),
    (9150, "Tor"),
    (7890, "Clash"),
    (7891, "Clash"),
    (7897, "Clash"),
    (9090, "Clash"),
    (10808, "V2Ray"),
    (10809, "V2Ray"),
    (2080, "sing-box"),
    (4444, "I2P"),
    (40000, "Cloudflare WARP"),
];

/// Tools whose ports are listened on locally, from `/proc/net/tcp*`, which
/// a Flatpak sandbox shares with the computer.
fn listening_tools() -> Vec<&'static str> {
    let mut found = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let mut fields = line.split_whitespace();
            let (Some(local), Some(_), Some(state)) = (fields.nth(1), fields.next(), fields.next())
            else {
                continue;
            };
            // 0A is LISTEN
            if state != "0A" {
                continue;
            }
            let Some(port) = local
                .rsplit_once(':')
                .and_then(|(_, p)| u16::from_str_radix(p, 16).ok())
            else {
                continue;
            };
            if let Some((_, tool)) = PORTS.iter().find(|(p, _)| *p == port) {
                if !found.contains(tool) {
                    found.push(tool);
                }
            }
        }
    }
    found
}

/// Whether programs are told to use a proxy (environment or desktop
/// settings).
fn system_proxy() -> bool {
    let from_env = [
        "all_proxy",
        "ALL_PROXY",
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
    ]
    .iter()
    .any(|v| std::env::var(v).is_ok_and(|s| !s.trim().is_empty()));
    from_env || {
        use gtk::{gio, prelude::*};
        gio::ProxyResolver::default()
            .lookup("https://example.org/", gio::Cancellable::NONE)
            .is_ok_and(|proxies| proxies.iter().any(|p| p.as_str() != "direct://"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_changes_and_partial_routes_are_distinguished() {
        let wifi = |family| Route {
            family,
            interface: Some("wifi".into()),
            tunnel: false,
        };
        let tunnel = |family| Route {
            family,
            interface: Some("Meta".into()),
            tunnel: true,
        };
        assert_eq!(classify_routes(&[]), Protection::Offline);
        assert!(matches!(
            classify_routes(&[wifi("IPv4"), wifi("IPv6")]),
            Protection::Exposed(_)
        ));
        let partial = classify_routes(&[tunnel("IPv4"), wifi("IPv6")]);
        assert_eq!(
            partial,
            Protection::Partial {
                tunnels: vec!["Meta".into()],
                exposed_families: vec!["IPv6"]
            }
        );
        assert!(!partial.is_protected());
        assert_eq!(partial.sharing_ipv4_only(), Some(true));
        assert_eq!(
            classify_routes(&[wifi("IPv4"), tunnel("IPv6")]),
            Protection::Partial {
                tunnels: vec!["Meta".into()],
                exposed_families: vec!["IPv4"]
            }
        );
        for routes in [vec![tunnel("IPv4"), tunnel("IPv6")]] {
            assert_eq!(
                classify_routes(&routes),
                Protection::Protected(vec!["Meta".into()])
            );
        }
        assert_eq!(
            classify_routes(&[tunnel("IPv4")]).sharing_ipv4_only(),
            Some(true)
        );
        assert_eq!(
            classify_routes(&[wifi("IPv4"), tunnel("IPv6")]).sharing_ipv4_only(),
            None
        );
        // A later check must reflect disconnection, not retain the tunnel.
        assert!(!classify_routes(&[wifi("IPv4")]).is_protected());
        assert!(!classify_routes(&[Route {
            family: "IPv4",
            interface: None,
            tunnel: false
        }])
        .is_protected());
    }

    #[test]
    fn this_computer_can_be_checked() {
        let status = status();
        println!("{status:?}");
        if let Protection::Protected(through) = &status.protection {
            assert!(!through.is_empty());
        }
    }

    #[test]
    fn interfaces_with_odd_names_are_not_tunnels() {
        assert!(!is_tunnel("../lo"));
        assert!(!is_tunnel(".hidden"));
        assert!(!is_tunnel("lo"));
        for name in ["utun0", "tun12", "tap3", "wg0"] {
            assert!(kernel_tunnel_name(name));
        }
        for name in [
            "en0",
            "em0",
            "ppp0",
            "gif0",
            "stf0",
            "utun",
            "tun0/evil",
            "wireguard",
        ] {
            assert!(!kernel_tunnel_name(name));
        }
    }
}
