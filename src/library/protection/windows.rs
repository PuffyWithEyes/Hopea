// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use std::net::{IpAddr, SocketAddr, UdpSocket};
use windows_sys::Win32::{NetworkManagement::IpHelper::*, Networking::WinSock::*};

fn address(ip: IpAddr) -> SOCKADDR_INET {
    match ip {
        IpAddr::V4(ip) => SOCKADDR_INET {
            Ipv4: SOCKADDR_IN {
                sin_family: AF_INET,
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from_ne_bytes(ip.octets()),
                    },
                },
                ..Default::default()
            },
        },
        IpAddr::V6(ip) => SOCKADDR_INET {
            Ipv6: SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 { Byte: ip.octets() },
                },
                ..Default::default()
            },
        },
    }
}

pub fn route(target: &str) -> Option<(String, bool)> {
    let target: SocketAddr = target.parse().ok()?;
    let socket = UdpSocket::bind(if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .ok()?;
    socket.connect(target).ok()?; // Chooses the route; sends no packets.
    let source = address(socket.local_addr().ok()?.ip());
    let destination = address(target.ip());
    let mut route = MIB_IPFORWARD_ROW2::default();
    let mut chosen_source = SOCKADDR_INET::default();
    let mut interface = MIB_IF_ROW2::default();
    // All pointers refer to initialized, appropriately aligned native structs.
    // An unknown route/driver never enables VPN-only sharing.
    unsafe {
        if GetBestRoute2(
            std::ptr::null(),
            0,
            &source,
            &destination,
            0,
            &mut route,
            &mut chosen_source,
        ) != 0
        {
            return Some((String::new(), false));
        }
        interface.InterfaceLuid = route.InterfaceLuid;
        if GetIfEntry2(&mut interface) != 0 {
            return Some((String::new(), false));
        }
    }
    let text = |value: &[u16]| {
        String::from_utf16_lossy(
            &value[..value.iter().position(|c| *c == 0).unwrap_or(value.len())],
        )
    };
    let description = text(&interface.Description).to_ascii_lowercase();
    // Driver description, not the freely editable connection name. Generic
    // virtual adapters and IPv6 transition tunnels are not evidence of a VPN.
    let tunnel = [
        "wireguard",
        "wintun",
        "openvpn",
        "tap-windows",
        "tailscale",
        "cloudflare warp",
        "wan miniport (ikev2)",
        "wan miniport (l2tp)",
        "wan miniport (sstp)",
        "wan miniport (pptp)",
    ]
    .iter()
    .any(|driver| description.contains(driver));
    Some((text(&interface.Alias), tunnel))
}
