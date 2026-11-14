// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use hopea_common::api::SpeedLimits;
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    ops::RangeInclusive,
    path::PathBuf,
    time::Duration,
};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// The port other nodes search this node on, unless configured otherwise.
/// If it is taken, a random one is used.
pub const DEFAULT_P2P_PORT: u16 = 47391;

/// Limits applied to every file while it is being validated, whether it was
/// shared here or downloaded from another node.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_upload_bytes: u64,
    /// Upper bound on decoded audio; protects against files that declare
    /// hours of audio in a few kilobytes
    pub max_duration: Duration,
    /// Wall clock budget for validating a single file
    pub max_validation_time: Duration,
    pub max_channels: u32,
    pub sample_rates: RangeInclusive<u32>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_upload_bytes: 512 * MIB,
            max_duration: Duration::from_secs(3 * 60 * 60),
            max_validation_time: Duration::from_secs(120),
            max_channels: 8,
            sample_rates: 8_000..=384_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub limits: Limits,
    /// Space for the tracks this node shares itself
    pub quota_bytes: u64,
    /// Space for tracks downloaded from others, which are kept and shared
    /// on until the least recently played ones make room
    pub cache_bytes: u64,
    /// Validation is CPU heavy; this many files are validated at once
    pub max_parallel_validations: usize,
    /// An upload that sends no data for this long is aborted
    pub upload_idle_timeout: Duration,

    /// Name shown next to the tracks this node shares
    pub name: Option<String>,
    /// Address of the local API used by the player
    pub listen: SocketAddr,
    /// Address other nodes search this node on
    pub p2p_listen: SocketAddr,
    /// BitTorrent listen port; 0 picks one
    pub bt_port: u16,
    /// Nodes that are always searched, in addition to those found in the DHT
    pub peers: Vec<SocketAddr>,
    /// Use the public BitTorrent DHT to find other nodes and peers
    pub dht: bool,
    /// Restrict all peer traffic (BitTorrent, DHT and HTTP) to IPv4.
    /// Used when the listener's VPN protects IPv4 but not IPv6.
    pub ipv4_only: bool,
    /// Ask the router to forward the BitTorrent and search ports
    pub upnp: bool,
    /// Find other nodes on the local network
    pub local_discovery: bool,
    /// How long a network search waits for answers
    pub search_timeout: Duration,
    /// How long playback waits for someone sharing a track to send data
    pub stream_timeout: Duration,
    /// Relay for nodes that others can't reach, when this one can be
    pub relay: bool,
    /// Nodes to try first as relays
    pub relays: Vec<SocketAddr>,
    /// Use a relay even if this node seems reachable
    pub force_relay: bool,
    /// Nodes relayed at once, at most
    pub max_relay_clients: usize,
    /// BitTorrent speed limits; made fair (see [`SpeedLimits::fair`]) before use
    pub speed: SpeedLimits,
}

impl Config {
    pub fn new(data_dir: PathBuf) -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self {
            data_dir,
            limits: Limits::default(),
            quota_bytes: 20 * GIB,
            cache_bytes: 5 * GIB,
            max_parallel_validations: (cpus / 2).clamp(1, 4),
            upload_idle_timeout: Duration::from_secs(60),
            name: None,
            listen: (Ipv4Addr::LOCALHOST, 0).into(),
            // IPv6 and IPv4 both, so that nodes behind an IPv4 NAT can still be
            // reached over IPv6
            p2p_listen: (Ipv6Addr::UNSPECIFIED, DEFAULT_P2P_PORT).into(),
            bt_port: 0,
            peers: Vec::new(),
            dht: true,
            ipv4_only: false,
            upnp: true,
            local_discovery: true,
            search_timeout: Duration::from_secs(8),
            stream_timeout: Duration::from_secs(45),
            relay: true,
            relays: Vec::new(),
            force_relay: false,
            max_relay_clients: 16,
            speed: SpeedLimits::default(),
        }
    }

    /// A node that only talks to explicitly configured peers on loopback,
    /// for tests.
    pub fn isolated(data_dir: PathBuf) -> Self {
        Self {
            p2p_listen: (Ipv4Addr::LOCALHOST, 0).into(),
            dht: false,
            upnp: false,
            local_discovery: false,
            search_timeout: Duration::from_secs(5),
            stream_timeout: Duration::from_secs(20),
            ..Self::new(data_dir)
        }
    }
}
