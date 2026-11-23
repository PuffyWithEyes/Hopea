// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Making sure other nodes can reach this one.
//!
//! Most computers sit behind a router that doesn't let connections in. The
//! node tries, in order:
//!
//! 1. **UPnP.** It asks the router to forward the search port (librqbit
//!    does the same for the BitTorrent port).
//! 2. **A check.** It asks a few other nodes to connect back to it
//!    ([`DIALBACK_PATH`]), once over IPv4 and once over IPv6. A node that
//!    passes either way is reachable directly. The search port listens on
//!    both, and the DHT announces both addresses on its own.
//! 3. **A relay.** Otherwise it finds a reachable node willing to relay
//!    (see [`crate::relay`]) and keeps a connection open to it, which the
//!    router allows since it goes out.
//!
//! The check is repeated every half hour, since addresses and routers
//! change.
//!
//! A node only ever connects back to the address the request came from, to
//! one port, to fetch its own hello: it can't be used to probe anyone else.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Query, State},
    http::StatusCode,
    Json,
};
use hopea_common::api::Reachability;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Semaphore};

use crate::{keywords, p2p, relay, AppState};

pub const HELLO_PATH: &str = "/p2p/v1/hello";
pub const DIALBACK_PATH: &str = "/p2p/v1/dialback";

/// How often reachability is checked again
const RECHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// When nobody could be asked, how soon to try again
const RETRY_INTERVAL: Duration = Duration::from_secs(2 * 60);
/// Time given to UPnP before the first check
const UPNP_GRACE: Duration = Duration::from_secs(5);
/// Nodes asked per address family
const HELPERS_PER_FAMILY: usize = 3;
const DIALBACK_TIMEOUT: Duration = Duration::from_secs(5);
/// Each address may ask for a dial-back this often
const DIALBACK_INTERVAL: Duration = Duration::from_secs(10);
const MAX_SMALL_RESPONSE: usize = 64 * 1024;

/// Every node that others can reach announces itself under this DHT key, so
/// that nodes can find each other to check reachability and to find relays.
pub fn nodes_key() -> [u8; 20] {
    keywords::dht_key("\0nodes")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// The node's public key
    pub node: String,
    pub bt_port: Option<u16>,
    /// Whether the node accepts relay clients right now
    pub relay: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DialbackResponse {
    /// The key of the node found at the requester's address, if any
    pub node: Option<String>,
}

/// What this node knows about its own reachability.
pub struct Status {
    inner: Mutex<(Reachability, Option<SocketAddr>, bool)>,
    changes: watch::Sender<Reachability>,
    /// When each address last asked for a dial-back
    dialbacks: Mutex<HashMap<IpAddr, Instant>>,
    dialback_slots: Semaphore,
}

impl Status {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new((Reachability::Checking, None, false)),
            changes: watch::channel(Reachability::Checking).0,
            dialbacks: Mutex::default(),
            dialback_slots: Semaphore::new(4),
        }
    }

    pub fn get(&self) -> Reachability {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).0
    }

    /// The relay in use, in relay mode.
    pub fn relay(&self) -> Option<SocketAddr> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    pub fn set(&self, reachability: Reachability, relay: Option<SocketAddr>) {
        self.replace(reachability, relay, true);
    }

    /// Observe the same confirmed state that the authenticated API reports.
    /// Bootstrap advertising is deliberately not an Online notification.
    pub fn subscribe(&self) -> watch::Receiver<Reachability> {
        self.changes.subscribe()
    }

    fn replace(&self, reachability: Reachability, relay: Option<SocketAddr>, confirmed: bool) {
        let mut status = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *status = (reachability, relay, confirmed);
        let reported = if confirmed {
            reachability
        } else {
            Reachability::Checking
        };
        // Publish while holding the state lock: a later transition must not be
        // followed by an older notification from another worker.
        self.changes.send_if_modified(|current| {
            if *current == reported {
                false
            } else {
                *current = reported;
                true
            }
        });
    }

    /// Public status must not mistake bootstrap optimism for a successful check.
    pub fn reported(&self) -> Reachability {
        let status = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if status.2 {
            status.0
        } else {
            Reachability::Checking
        }
    }

    pub(crate) fn bootstrap(&self) {
        self.replace(Reachability::Direct, None, false);
    }

    /// Whether other nodes can connect to this one directly.
    pub fn direct(&self) -> bool {
        matches!(self.get(), Reachability::Direct | Reachability::Ipv6)
    }
}

pub async fn hello_handler(State(state): State<AppState>) -> Json<Hello> {
    Json(hello(&state))
}

pub fn hello(state: &AppState) -> Hello {
    Hello {
        node: state.identity.public_hex(),
        bt_port: state.torrents.bt_port(),
        relay: state.relay.accepting(state),
    }
}

#[derive(Deserialize)]
pub struct DialbackQuery {
    port: u16,
}

/// Connects back to the requester's own address on the given port and
/// reports which node, if any, answered there.
pub async fn dialback_handler(
    State(state): State<AppState>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Query(query): Query<DialbackQuery>,
) -> Result<Json<DialbackResponse>, StatusCode> {
    if query.port < 1024 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ip = from.ip().to_canonical();
    // Nodes on the same computer (tests, or several nodes on one machine)
    // share an address, and can't be used to reach anyone else anyway
    if !ip.is_loopback() {
        let mut recent = state
            .reach
            .dialbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        recent.retain(|_, at| now.duration_since(*at) < DIALBACK_INTERVAL);
        if recent.contains_key(&ip) || recent.len() > 1000 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        recent.insert(ip, now);
    }
    let _slot = state
        .reach
        .dialback_slots
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;

    let node = fetch_hello(&state, SocketAddr::new(ip, query.port))
        .await
        .ok()
        .map(|h| h.node);
    Ok(Json(DialbackResponse { node }))
}

/// Reads a small JSON response, refusing anything large.
pub async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    max: usize,
) -> anyhow::Result<T> {
    let response_status = response.status();
    if !response_status.is_success() {
        anyhow::bail!("status {response_status}");
    }
    if response.content_length().unwrap_or(0) > max as u64 {
        anyhow::bail!("response too large");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > max.saturating_sub(body.len()) {
            anyhow::bail!("response too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}

pub async fn fetch_hello(state: &AppState, addr: SocketAddr) -> anyhow::Result<Hello> {
    let response = state
        .http
        .get(format!("http://{addr}{HELLO_PATH}"))
        .timeout(DIALBACK_TIMEOUT)
        .send()
        .await?;
    read_json(response, MAX_SMALL_RESPONSE).await
}

/// Asks `helper` to connect back to this node's search port.
async fn dialback(state: &AppState, helper: SocketAddr, port: u16) -> anyhow::Result<bool> {
    let response = state
        .http
        .get(format!("http://{helper}{DIALBACK_PATH}?port={port}"))
        .timeout(DIALBACK_TIMEOUT * 2)
        .send()
        .await?;
    let answer: DialbackResponse = read_json(response, MAX_SMALL_RESPONSE).await?;
    Ok(answer.node.as_deref() == Some(state.identity.public_hex().as_str()))
}

/// Other nodes: configured peers and relays, then reachable nodes from the
/// DHT. Never this node itself.
pub async fn other_nodes(state: &AppState) -> Vec<SocketAddr> {
    let mut nodes: Vec<SocketAddr> = state
        .config
        .relays
        .iter()
        .chain(&state.config.peers)
        .copied()
        .collect();
    for addr in p2p::dht_lookup(state, nodes_key(), None, Duration::from_secs(5), 50).await {
        if !nodes.contains(&addr) {
            nodes.push(addr);
        }
    }
    let own = state.p2p_addr();
    let mut seen = std::collections::HashSet::new();
    nodes.retain(|a| Some(*a) != own && seen.insert(*a));
    nodes.truncate(64);
    nodes
}

/// Checks whether others can reach this node over IPv4 and over IPv6.
/// `None` for a family means no other node could be asked.
async fn check(
    state: &AppState,
    nodes: &[SocketAddr],
    round: usize,
) -> (Option<bool>, Option<bool>) {
    let Some(port) = state.p2p_addr().map(|a| a.port()) else {
        return (None, None);
    };
    let family = |v6: bool| async move {
        if v6 && state.config.ipv4_only {
            return None;
        }
        let helpers = helpers_for_round(nodes, v6, round);
        let answers = futures_util::future::join_all(
            helpers
                .into_iter()
                .map(|helper| async move { dialback(state, helper, port).await }),
        )
        .await;
        let mut asked = false;
        for answer in answers {
            match answer {
                Ok(true) => return Some(true),
                Ok(false) => asked = true,
                Err(err) => tracing::debug!("Reachability check failed: {err:#}"),
            }
        }
        asked.then_some(false)
    };
    tokio::join!(family(false), family(true))
}

fn helpers_for_round(nodes: &[SocketAddr], v6: bool, round: usize) -> Vec<SocketAddr> {
    let family: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|a| a.is_ipv6() == v6)
        .collect();
    if family.is_empty() {
        return Vec::new();
    }
    let start = round.wrapping_mul(HELPERS_PER_FAMILY) % family.len();
    family
        .iter()
        .cycle()
        .skip(start)
        .take(HELPERS_PER_FAMILY.min(family.len()))
        .copied()
        .collect()
}

/// Forwards the search port with UPnP for as long as the node runs.
pub async fn upnp(state: AppState) {
    let Some(port) = state.p2p_addr().map(|a| a.port()) else {
        return;
    };
    match librqbit_upnp::UpnpPortForwarder::new(vec![port], None, None) {
        Ok(forwarder) => {
            tracing::debug!("Asking the router to forward port {port}");
            forwarder.run_forever().await
        }
        Err(err) => tracing::debug!("UPnP unavailable: {err:#}"),
    }
}

/// Keeps the reachability status current, and falls back to a relay when
/// nothing else works.
pub async fn run(state: AppState) {
    if state.config.upnp {
        tokio::time::sleep(UPNP_GRACE).await;
    }
    if state.config.dht {
        p2p::wait_for_dht(&state, Duration::from_secs(30)).await;
    }
    let mut round = 0usize;
    loop {
        let nodes = other_nodes(&state).await;
        let (v4, v6) = if state.config.force_relay {
            (Some(false), Some(false))
        } else {
            check(&state, &nodes, round).await
        };
        round = round.wrapping_add(1);
        let before = (state.reach.get(), state.reach.reported());
        let next_check = match (v4, v6) {
            (Some(true), _) => {
                state.reach.set(Reachability::Direct, None);
                RECHECK_INTERVAL
            }
            (_, Some(true)) => {
                state.reach.set(Reachability::Ipv6, None);
                RECHECK_INTERVAL
            }
            (None, None) => {
                // Nobody to ask: most likely the very first nodes of a
                // network. Behave as reachable, and ask again soon
                tracing::debug!("No other nodes to check reachability with");
                state.reach.bootstrap();
                if !nodes.is_empty() {
                    // A helper may answer hello/register but not dialback.
                    // An inconclusive check must not prevent trying a relay.
                    state.announce.notify_one();
                    relay::run_client(&state, &nodes, RECHECK_INTERVAL, round.wrapping_sub(1))
                        .await;
                    continue;
                }
                RETRY_INTERVAL
            }
            _ => {
                tracing::info!("Other nodes can't reach this one; looking for a relay");
                state.reach.set(Reachability::Unreachable, None);
                state.announce.notify_one();
                relay::run_client(&state, &nodes, RECHECK_INTERVAL, round.wrapping_sub(1)).await;
                continue;
            }
        };
        let now = (state.reach.get(), state.reach.reported());
        if now.1 != before.1 {
            tracing::info!("Reachability: {:?}", now.1);
        }
        if now != before {
            state.announce.notify_one();
        }
        tokio::time::sleep(next_check).await;
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn confirmation_notifies_observers_even_after_optimistic_bootstrap() {
        let status = Status::new();
        let mut changes = status.subscribe();
        status.bootstrap();
        assert_eq!(status.get(), Reachability::Direct);
        assert!(!changes.has_changed().unwrap());

        status.set(Reachability::Direct, None);
        assert!(changes.has_changed().unwrap());
        assert_eq!(*changes.borrow_and_update(), Reachability::Direct);
        assert_eq!(status.reported(), Reachability::Direct);
        status.set(Reachability::Direct, None);
        assert!(
            !changes.has_changed().unwrap(),
            "unchanged checks must not poll the UI again"
        );

        status.set(
            Reachability::Relay,
            Some(SocketAddr::from(([8, 8, 8, 8], 47391))),
        );
        assert_eq!(*changes.borrow_and_update(), Reachability::Relay);
        status.bootstrap();
        assert!(changes.has_changed().unwrap());
        assert_eq!(*changes.borrow_and_update(), Reachability::Checking);
    }

    #[test]
    fn retry_rounds_do_not_keep_asking_only_the_first_helpers() {
        let nodes: Vec<SocketAddr> = (1..=7)
            .map(|n| SocketAddr::from(([8, 8, 8, n], 47391)))
            .collect();
        assert_eq!(helpers_for_round(&nodes, false, 0), nodes[..3]);
        assert_eq!(helpers_for_round(&nodes, false, 1), nodes[3..6]);
        assert_eq!(
            helpers_for_round(&nodes, false, 2),
            vec![nodes[6], nodes[0], nodes[1]]
        );
        assert!(helpers_for_round(&nodes, true, 0).is_empty());
        assert_eq!(helpers_for_round(&nodes[..1], false, 10), nodes[..1]);
    }

    #[test]
    fn bootstrap_is_not_confirmed_reachability() {
        let status = Status::new();
        assert_eq!(status.reported(), Reachability::Checking);
        status.bootstrap();
        assert!(status.direct(), "bootstrap discovery must remain enabled");
        assert_eq!(status.reported(), Reachability::Checking);
        for reach in [
            Reachability::Direct,
            Reachability::Ipv6,
            Reachability::Relay,
            Reachability::Unreachable,
        ] {
            status.set(reach, None);
            assert_eq!(status.reported(), reach);
        }
        status.bootstrap();
        assert_eq!(status.reported(), Reachability::Checking);
    }
}
