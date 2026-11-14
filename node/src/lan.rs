// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! DNS-SD discovers search endpoints; BitTorrent LSD alone only discovers
//! peers for torrents whose hashes we already know. No catalog or credentials
//! are broadcast. Discovered endpoints still go through signed-entry validation.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::Mutex,
};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::AppState;

const SERVICE: &str = "_hopea._tcp.local.";
const MAX_SERVICES: usize = 128;

#[derive(Default)]
pub struct Peers(Mutex<HashMap<String, Vec<SocketAddr>>>);

impl Peers {
    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.0
            .lock()
            .unwrap()
            .values()
            .flatten()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
}

struct Daemon(ServiceDaemon);

impl Drop for Daemon {
    fn drop(&mut self) {
        // Also runs on task cancellation: do not leave a daemon advertising
        // a closed listener after sharing is disabled or the VPN disappears.
        let _ = self.0.shutdown();
    }
}

pub async fn run(state: AppState) {
    if let Err(err) = discover(&state).await {
        tracing::warn!("Local music discovery unavailable: {err}");
    }
    state.lan.0.lock().unwrap().clear();
}

async fn discover(state: &AppState) -> anyhow::Result<()> {
    let Some(listen) = state.p2p_addr() else {
        return Ok(());
    };
    let daemon = Daemon(ServiceDaemon::new()?);
    let identity = state.identity.public_hex();
    // A DNS label is at most 63 bytes. The TXT record carries the full key.
    let instance = format!("hopea-{}", &identity[..32]);
    let hostname = format!("{instance}.local.");
    let addresses = if listen.ip().is_unspecified() {
        String::new()
    } else {
        listen.ip().to_string()
    };
    let mut service = ServiceInfo::new(
        SERVICE,
        &instance,
        &hostname,
        addresses.as_str(),
        listen.port(),
        &[("node", identity.as_str()), ("version", "1")][..],
    )?;
    if listen.ip().is_unspecified() {
        service = service.enable_addr_auto();
    }
    daemon.0.set_ip_check_interval(5)?;
    daemon.0.register(service)?;
    let events = daemon.0.browse(SERVICE)?;
    while let Ok(event) = events.recv_async().await {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                if info.get_property_val_str("node") == Some(identity.as_str())
                    || info.get_property_val_str("version") != Some("1")
                    || info.get_port() == 0
                {
                    continue;
                }
                let addresses: Vec<_> = info
                    .get_addresses()
                    .iter()
                    .map(|ip| ip.to_ip_addr().to_canonical())
                    .filter(|ip| usable_address(*ip))
                    .filter(|ip| !listen.is_ipv4() || ip.is_ipv4())
                    .take(8)
                    .map(|ip| SocketAddr::new(ip, info.get_port()))
                    .collect();
                let mut peers = state.lan.0.lock().unwrap();
                if peers.len() < MAX_SERVICES || peers.contains_key(info.get_fullname()) {
                    peers.insert(info.get_fullname().to_string(), addresses);
                }
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                state.lan.0.lock().unwrap().remove(&name);
            }
            _ => {}
        }
    }
    Ok(())
}

fn usable_address(ip: IpAddr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(ip) => !ip.is_broadcast(),
        // reqwest cannot connect to zone-qualified IPv6 URLs. Keep IPv4 and
        // routable IPv6 candidates, rather than losing the interface scope.
        IpAddr::V6(ip) => !ip.is_unicast_link_local(),
    }
}
