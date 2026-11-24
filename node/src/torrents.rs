// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The BitTorrent side of the node, on top of librqbit.
//!
//! Each track is its own single-file torrent, stored in its own folder
//! named after the info hash. Torrents are only ever added from info
//! dictionaries that passed [`crate::torrent_info::InfoDict::parse`] (or
//! that this node built), so the library never sees a name or path from
//! anyone else.

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Context;
use librqbit::{
    dht::{Dht, Id20},
    AddTorrent, AddTorrentOptions, DhtSessionConfig, ListenerMode, ListenerOptions, ManagedTorrent,
    Session, SessionOptions,
};

use hopea_common::api::SpeedLimits;

use crate::{config::Config, storage::Storage, torrent_info};

pub type Handle = Arc<ManagedTorrent>;

pub struct Torrents {
    session: Arc<Session>,
    storage: Storage,
    policy: std::sync::Mutex<(SpeedLimits, bool)>,
}

fn to_rqbit(limits: SpeedLimits) -> librqbit::limits::LimitsConfig {
    librqbit::limits::LimitsConfig {
        download_bps: limits.download.and_then(std::num::NonZeroU32::new),
        upload_bps: limits.upload.and_then(std::num::NonZeroU32::new),
    }
}

pub fn id20(info_hash: &str) -> anyhow::Result<Id20> {
    let bytes: [u8; 20] = hex::decode(info_hash)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("not an info hash"))?;
    Ok(Id20::new(bytes))
}

impl Torrents {
    pub async fn start(config: &Config, storage: Storage) -> anyhow::Result<Self> {
        let dht = config.dht.then(|| DhtSessionConfig {
            persistence: Some(librqbit::dht::DhtPersistenceConfig {
                dump_interval: None,
                config_filename: Some(storage.session_dir().join("dht.json")),
            }),
            ..Default::default()
        });
        let ipv4_only = config.ipv4_only || !config.dht;
        let listen_ip: IpAddr = if config.dht && !ipv4_only {
            // Both IPv4 and IPv6
            Ipv6Addr::UNSPECIFIED.into()
        } else if !config.dht && !config.local_discovery {
            // Isolated nodes (tests) stay on loopback
            Ipv4Addr::LOCALHOST.into()
        } else {
            Ipv4Addr::UNSPECIFIED.into()
        };
        let options = SessionOptions {
            dht,
            disable_trackers: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                listen_addr: (listen_ip, config.bt_port).into(),
                enable_upnp_port_forwarding: config.upnp,
                ipv4_only,
                max_pending_incoming_handshake_checks: 64,
                ..Default::default()
            }),
            disable_local_service_discovery: !config.local_discovery,
            ratelimits: to_rqbit(config.speed.fair()),
            peer_limit: Some(64),
            ipv4_only,
            client_name_and_version: Some(format!("hopea {}", env!("CARGO_PKG_VERSION"))),
            ..Default::default()
        };
        let session = Session::new_with_opts(storage.session_dir(), options)
            .await
            .context("unable to start the BitTorrent session")?;
        Ok(Self {
            session,
            storage,
            policy: std::sync::Mutex::new((config.speed.fair(), false)),
        })
    }

    /// Charge HTTP audio against the same live quota as BitTorrent traffic.
    pub(crate) async fn charge_http(&self, mut bytes: usize, upload: bool) -> anyhow::Result<()> {
        while bytes > 0 {
            let cap = if upload {
                self.limits().upload
            } else {
                self.limits().download
            };
            let part = bytes
                .min(cap.unwrap_or(u32::MAX).max(1) as usize)
                .min(16 * 1024);
            let size = std::num::NonZeroU32::new(part as u32).unwrap();
            if upload {
                self.session.ratelimits.prepare_for_upload(size).await?;
            } else {
                self.session.ratelimits.prepare_for_download(size).await?;
            }
            bytes -= part;
        }
        Ok(())
    }

    /// The speed limits in effect.
    pub fn limits(&self) -> SpeedLimits {
        SpeedLimits {
            download: self.session.ratelimits.get_download_bps().map(|b| b.get()),
            upload: self.session.ratelimits.get_upload_bps().map(|b| b.get()),
        }
    }

    /// Changes the speed limits right away, after making them fair. Returns
    /// the limits in effect.
    pub fn set_limits(&self, limits: SpeedLimits) -> SpeedLimits {
        let mut policy = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        policy.0 = limits.fair();
        self.apply_policy(*policy)
    }

    pub fn set_gaming(&self, active: bool) {
        let mut policy = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        policy.1 = active;
        self.apply_policy(*policy);
    }

    pub fn gaming(&self) -> bool {
        self.policy.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    fn apply_policy(&self, (requested, gaming): (SpeedLimits, bool)) -> SpeedLimits {
        let effective = if gaming {
            requested.during_game()
        } else {
            requested
        };
        let limits = to_rqbit(effective);
        self.session
            .ratelimits
            .set_download_bps(limits.download_bps);
        self.session.ratelimits.set_upload_bps(limits.upload_bps);
        self.limits()
    }

    /// The port other peers reach this node's BitTorrent listener on.
    pub fn bt_port(&self) -> Option<u16> {
        self.session
            .announce_port()
            .or_else(|| self.session.listen_addr().map(|a| a.port()))
    }

    pub fn dht(&self) -> Option<Dht> {
        self.session.get_dht().cloned()
    }

    pub fn get(&self, info_hash: &str) -> Option<Handle> {
        self.session.get(id20(info_hash).ok()?.into())
    }

    /// Adds a track's torrent, to share a file that is already in place or
    /// to start downloading one. `peers` are nodes known to have it.
    /// Adding a torrent that is already in the session returns it.
    pub async fn add(
        &self,
        info_hash: &str,
        info_bytes: &[u8],
        peers: Vec<SocketAddr>,
    ) -> anyhow::Result<Handle> {
        if let Some(handle) = self.get(info_hash) {
            return Ok(handle);
        }
        let folder = self.storage.media_dir(info_hash);
        std::fs::create_dir_all(&folder)?;
        let peers: Vec<SocketAddr> = peers
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let response = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(torrent_info::torrent_file(info_bytes)),
                Some(AddTorrentOptions {
                    output_folder: Some(
                        folder
                            .to_str()
                            .context("the data directory is not valid UTF-8")?
                            .to_string(),
                    ),
                    // Needed to share a file that is already complete
                    overwrite: true,
                    initial_peers: (!peers.is_empty()).then_some(peers),
                    ..Default::default()
                }),
            )
            .await?;
        response.into_handle().context("the torrent was not added")
    }

    /// Stops sharing a track. Its files are left to the caller.
    pub async fn remove(&self, info_hash: &str) {
        if let Ok(id) = id20(info_hash) {
            if self.session.get(id.into()).is_some() {
                if let Err(err) = self.session.delete(id.into(), false).await {
                    tracing::warn!("Unable to remove torrent {info_hash}: {err:#}");
                }
            }
        }
    }

    pub async fn stop(&self) {
        self.session.stop().await;
    }
}
