// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Diagnose local discovery without opening a library or advertising music.
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let daemon = mdns_sd::ServiceDaemon::new()?;
    let events = daemon.browse("_hopea._tcp.local.")?;
    let until = Instant::now() + Duration::from_secs(8);
    while let Some(remaining) = until.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(remaining) else {
            break;
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(service) = event {
            for ip in service.get_addresses() {
                println!(
                    "{}",
                    std::net::SocketAddr::new(ip.to_ip_addr(), service.get_port())
                );
            }
        }
    }
    let _ = daemon.shutdown();
    Ok(())
}
