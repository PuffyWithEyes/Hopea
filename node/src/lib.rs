// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Peer-to-peer music sharing, built into the player.
//!
//! Every player runs a node. A node shares the tracks its owner adds over
//! BitTorrent, one torrent per track, and keeps nothing else about the
//! network: no server, no tracker, no shared catalog. Tracks are found by
//! asking the nodes that announced matching words in the BitTorrent DHT
//! (see [`p2p`] and [`keywords`]). Whoever downloads a track checks it,
//! keeps it, and shares it on.
//!
//! Everything a node receives from others is untrusted. Search answers are
//! signed entries whose every field is checked ([`entry`]); torrents are
//! only accepted in one strict shape ([`torrent_info`]); downloads must
//! survive the same validation as uploads ([`ingest`]); and the player only
//! ever receives PCM decoded here, never another person's file
//! ([`stream`]).
//!
//! The player talks to its node over an HTTP API on loopback ([`api`]).

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod entry;
pub mod fsx;
pub mod fuzzy;
pub mod identity;
pub mod ingest;
mod ingress;
pub mod keywords;
pub mod lan;
pub mod library;
pub mod lyrics;
pub mod metadata;
pub mod p2p;
pub mod reach;
pub mod recommend;
pub mod relay;
pub mod storage;
pub mod stream;
pub mod torrent_info;
pub mod torrents;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use anyhow::Context;
use tokio::{net::TcpListener, sync::Semaphore, task::JoinHandle};

use crate::{config::Config, db::Db, identity::Identity, storage::Storage, torrents::Torrents};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub storage: Storage,
    pub identity: Arc<Identity>,
    pub torrents: Arc<Torrents>,
    pub http: reqwest::Client,
    /// Bounds CPU heavy validation of uploads and downloads
    pub validations: Arc<Semaphore>,
    /// Bounds uploads received at once
    pub uploads: Arc<Semaphore>,
    /// Bounds network searches running at once
    pub searches: Arc<Semaphore>,
    /// Rotate bounded discovery candidates between attempts.
    pub(crate) search_round: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) reach_round: Arc<std::sync::atomic::AtomicUsize>,
    /// Wakes the DHT announcer when the set of tracks changed
    pub announce: Arc<tokio::sync::Notify>,
    pub token_hash: [u8; 32],
    pub last_recommendation_search: Arc<Mutex<Option<Instant>>>,
    pub recommendation_check: Arc<tokio::sync::Mutex<Option<Instant>>>,
    pub share_work: Arc<tokio::sync::Mutex<()>>,
    pub metadata_work: Arc<tokio::sync::Mutex<()>>,
    pub lyrics_work: Arc<tokio::sync::Mutex<()>>,
    /// Whether and how other nodes reach this one
    pub reach: Arc<reach::Status>,
    /// Relaying for nodes nobody can reach
    pub relay: Arc<relay::Server>,
    pub lan: Arc<lan::Peers>,
    /// Bumped whenever the set of tracks this node has changes
    pub changes: Arc<tokio::sync::watch::Sender<u64>>,
    /// Pictures chosen for uploads still to come: `(SHA-256, JPEG)`
    pub pending_covers: Arc<Mutex<std::collections::VecDeque<(String, Vec<u8>)>>>,
    p2p_addr: Arc<OnceLock<SocketAddr>>,
}

impl AppState {
    /// Opens the data directory and starts the BitTorrent session.
    pub async fn new(mut config: Config, token: &str) -> anyhow::Result<Self> {
        if config.ipv4_only {
            // The default listener is dual-stack. Never leave it listening
            // on IPv6 when the application selected an IPv4-only VPN route.
            if config.p2p_listen.ip().is_unspecified() {
                config
                    .p2p_listen
                    .set_ip(std::net::Ipv4Addr::UNSPECIFIED.into());
            }
            anyhow::ensure!(
                config.p2p_listen.is_ipv4(),
                "IPv4-only sharing needs an IPv4 listener"
            );
            config.peers.retain(SocketAddr::is_ipv4);
            config.relays.retain(SocketAddr::is_ipv4);
            config.upnp = false;
            config.local_discovery = false;
        }
        let storage =
            Storage::init(&config.data_dir).context("unable to set up the data directory")?;
        let identity = Identity::load_or_create(&storage.identity_path())
            .context("unable to load this node's key")?;
        let db = Db::open(&storage.db_path(), &identity.public_hex())?;
        let torrents = Torrents::start(&config, storage.clone()).await?;
        let mut http = reqwest::Client::builder()
            .user_agent(concat!("hopea/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if config.ipv4_only {
            // Hyper filters remote addresses to this local address family,
            // including literal IPs and addresses received from older peers.
            http = http.local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        }
        let http = http.build()?;
        Ok(Self {
            validations: Arc::new(Semaphore::new(config.max_parallel_validations)),
            uploads: Arc::new(Semaphore::new(2)),
            searches: Arc::new(Semaphore::new(p2p::PARALLEL_SEARCHES)),
            search_round: Arc::default(),
            reach_round: Arc::default(),
            config: Arc::new(config),
            db,
            storage,
            identity: Arc::new(identity),
            torrents: Arc::new(torrents),
            http,
            announce: Arc::new(tokio::sync::Notify::new()),
            token_hash: auth::hash_token(token),
            last_recommendation_search: Arc::default(),
            recommendation_check: Arc::default(),
            lyrics_work: Arc::default(),
            metadata_work: Arc::default(),
            share_work: Arc::default(),
            reach: Arc::new(reach::Status::new()),
            relay: Arc::default(),
            lan: Arc::default(),
            changes: Arc::new(tokio::sync::watch::Sender::new(0)),
            pending_covers: Arc::default(),
            p2p_addr: Arc::default(),
        })
    }

    /// The player's API.
    pub fn router(&self) -> axum::Router {
        api::router(self.clone())
    }

    /// Tells the DHT announcer and the relay (if this node uses one) that
    /// the tracks this node has changed.
    pub fn tracks_changed(&self) {
        self.announce.notify_one();
        self.changes.send_modify(|v| *v += 1);
    }

    /// Where other nodes search this one, once listening.
    pub fn p2p_addr(&self) -> Option<SocketAddr> {
        self.p2p_addr.get().copied()
    }
}

/// A running node.
pub struct Node {
    pub state: AppState,
    /// Where the player's API listens
    pub api_addr: SocketAddr,
    /// Where other nodes search this one
    pub p2p_addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Node {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Node {
    /// Starts the BitTorrent session, the player's API, the search endpoint
    /// for other nodes, and the background work.
    pub async fn start(config: Config, token: &str) -> anyhow::Result<Self> {
        let state = AppState::new(config, token).await?;
        let config = state.config.clone();

        let api = TcpListener::bind(config.listen)
            .await
            .with_context(|| format!("unable to listen on {}", config.listen))?;
        let api_addr = api.local_addr()?;

        // The usual port keeps this node's DHT announces valid across
        // restarts; if something else has it, any port will do
        let p2p = match TcpListener::bind(config.p2p_listen).await {
            Ok(listener) => listener,
            Err(err) if config.p2p_listen.port() != 0 => {
                tracing::warn!(
                    "Port {} is taken ({err}); using another",
                    config.p2p_listen.port()
                );
                TcpListener::bind(SocketAddr::new(config.p2p_listen.ip(), 0)).await?
            }
            Err(err) => return Err(err.into()),
        };
        let p2p_addr = p2p.local_addr()?;
        let _ = state.p2p_addr.set(p2p_addr);
        tracing::info!(
            "Node {} listening for the player on {api_addr}, for searches on {p2p_addr}, BitTorrent port {:?}",
            identity::fingerprint(&state.identity.public_hex()),
            state.torrents.bt_port(),
        );

        let router = state.router();
        let p2p_router = p2p::router(state.clone());
        let tasks = vec![
            tokio::spawn(async move {
                if let Err(err) = axum::serve(api, router).await {
                    tracing::error!("The player API stopped: {err}");
                }
            }),
            tokio::spawn(async move {
                let service = p2p_router.into_make_service_with_connect_info::<ingress::PeerAddr>();
                if let Err(err) = axum::serve(ingress::Listener::new(p2p), service).await {
                    tracing::error!("The search endpoint stopped: {err}");
                }
            }),
            tokio::spawn(library::resume(state.clone())),
            tokio::spawn(p2p::announce_loop(state.clone())),
            tokio::spawn(reach::run(state.clone())),
            tokio::spawn(relay::maintain(state.clone())),
        ];
        let mut tasks = tasks;
        if config.local_discovery {
            tasks.push(tokio::spawn(lan::run(state.clone())));
        }
        if config.upnp {
            tasks.push(tokio::spawn(reach::upnp(state.clone())));
        }

        Ok(Self {
            state,
            api_addr,
            p2p_addr,
            tasks,
        })
    }

    pub async fn shutdown(self) {
        for task in &self.tasks {
            task.abort();
        }
        self.state.torrents.stop().await;
    }
}
