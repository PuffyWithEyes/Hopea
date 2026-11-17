// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Searching other nodes, and being searched.
//!
//! Every node answers `GET /p2p/v1/search?q=…` with the signed entries of
//! matching tracks it has the files of. To be found, it announces itself in
//! the BitTorrent DHT under the words of those tracks (see
//! [`crate::keywords`]). A search looks the query's words up in the DHT,
//! asks the nodes found there, and keeps what they send after checking
//! every entry. The nodes that answered are remembered as sources for the
//! BitTorrent download.
//!
//! The same public port also serves reachability checks
//! ([`crate::reach`]) and relaying ([`crate::relay`]). It is the only thing
//! this node exposes besides BitTorrent itself; the player's API stays on
//! loopback.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    time::Duration,
};
use tokio::time::Instant;

use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{header, HeaderValue},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{stream, StreamExt};
use hopea_common::api::{DhtState, SearchFailure, SearchReport};
use serde::{Deserialize, Serialize};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::{
    db::{now, Source, State as TrackState},
    entry::{Entry, WireEntry},
    keywords,
    reach::{self, read_json},
    relay, AppState,
};

pub const SEARCH_PATH: &str = "/p2p/v1/search";
pub const INFO_PATH: &str = "/p2p/v1/info";
pub const COVER_PATH: &str = "/p2p/v1/cover";
/// Results one node returns at most
const MAX_RESULTS: u32 = 50;
/// Largest search answer read. Answers carry no info dictionaries, about
/// 1.5 KiB per result, so this leaves plenty of room
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Nodes asked at most per search
const MAX_NODES_PER_SEARCH: usize = 40;
/// Nodes asked at once during a search. Each answer is stored as soon as it
/// arrives, so this bounds the memory a search takes
const PARALLEL_SEARCH_REQUESTS: usize = 8;
/// Network searches running at once; others wait their turn
pub const PARALLEL_SEARCHES: usize = 3;
/// How long DHT lookups for a search run
const DHT_LOOKUP_TIME: Duration = Duration::from_secs(4);
/// How often this node announces its words. DHT nodes forget announces
/// after about half an hour
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(24 * 60);
/// How long one announce lookup is given to finish
const ANNOUNCE_LOOKUP_TIME: Duration = Duration::from_secs(15);
// At most 1,401 keys (own + relayed + node discovery), each up to 15 s.
// Sixteen slots can sweep them within 22 minutes, below the refresh period.
// Do not reduce this during games: stale announcements make shared music vanish.
const PARALLEL_ANNOUNCES: usize = 16;
/// Nodes the DHT must know before lookups are worth making
const DHT_READY_NODES: usize = 8;
/// After starting, words are announced again sooner than usual: the first
/// announces reach few DHT nodes while the node is still finding its way
const EARLY_ANNOUNCE_INTERVALS: [Duration; 2] =
    [Duration::from_secs(2 * 60), Duration::from_secs(10 * 60)];

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    /// The answering node's key, so that a node can tell its own answers
    pub node: String,
    /// Where the answering node accepts BitTorrent connections
    pub bt_port: Option<u16>,
    #[serde(deserialize_with = "bounded_results")]
    pub entries: Vec<WireEntry>,
    /// Info hashes of the entries that came from the answering node's relay
    /// clients: they have to be fetched through it ([`relay::FETCH_PATH`])
    #[serde(default, deserialize_with = "bounded_results")]
    pub relayed: Vec<String>,
}

/// Bound element counts during decoding, not after allocating a huge Vec.
fn bounded_results<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("at most 100 search results")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = seq.next_element()? {
                if entries.len() == 2 * MAX_RESULTS as usize {
                    return Err(serde::de::Error::custom("too many search results"));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_seq(Visitor(std::marker::PhantomData))
}

#[cfg(test)]
mod hostile_answers {
    use super::*;
    #[test]
    fn rejects_excess_elements_during_json_decoding() {
        let entry = r#"{"body":"","sig":""}"#;
        let body = format!(
            r#"{{"node":"test","bt_port":null,"entries":[{}]}}"#,
            vec![entry; 101].join(",")
        );
        assert!(serde_json::from_str::<SearchResponse>(&body)
            .unwrap_err()
            .to_string()
            .contains("too many search results"));
        let body = format!(
            r#"{{"node":"test","bt_port":null,"entries":[],"relayed":[{}]}}"#,
            vec![r#""x""#; 101].join(",")
        );
        assert!(serde_json::from_str::<SearchResponse>(&body).is_err());
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(SEARCH_PATH, get(search_handler))
        .route(INFO_PATH, get(info_handler))
        .route(COVER_PATH, get(cover_handler))
        .route(crate::lyrics::PATH, get(crate::lyrics::serve))
        .route(crate::metadata::PATH, get(crate::metadata::serve))
        .route(crate::metadata::DATA_PATH, get(crate::metadata::serve_data))
        .route(
            relay::LYRICS_PATH,
            post(relay::lyrics_handler).layer(DefaultBodyLimit::max(crate::lyrics::MAX_WIRE)),
        )
        .route(
            relay::METADATA_PATH,
            post(relay::metadata_handler)
                .layer(DefaultBodyLimit::max(crate::metadata::MAX_DATA * 2)),
        )
        .route(reach::HELLO_PATH, get(reach::hello_handler))
        .route(reach::DIALBACK_PATH, get(reach::dialback_handler))
        .route(relay::REGISTER_PATH, post(relay::register_handler))
        .route(relay::POLL_PATH, get(relay::poll_handler))
        .route(
            relay::ANSWER_PATH,
            post(relay::answer_handler).layer(DefaultBodyLimit::max(relay::MAX_ANSWER_BODY)),
        )
        .route(
            relay::UPLOAD_PATH,
            // The size is checked against the torrent while receiving
            post(relay::upload_handler).layer(DefaultBodyLimit::disable()),
        )
        .route(relay::FETCH_PATH, post(relay::fetch_handler))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            crate::ingress::Guard::new(),
            crate::ingress::protect,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .with_state(state)
}

/// What this node itself answers to a search. Info dictionaries are only
/// sent along to a relay, which needs them to check what it fetches.
pub async fn local_answer(state: &AppState, query: &str, with_info: bool) -> SearchResponse {
    let q: String = query.chars().take(200).collect();
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || db.search_held(&q, MAX_RESULTS))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    SearchResponse {
        node: state.identity.public_hex(),
        bt_port: state.torrents.bt_port(),
        entries: rows
            .into_iter()
            .map(|(body, sig, info)| WireEntry {
                body: STANDARD.encode(body),
                sig: STANDARD.encode(sig),
                info: (with_info && !info.is_empty()).then(|| STANDARD.encode(info)),
            })
            .collect(),
        relayed: Vec::new(),
    }
}

async fn search_handler(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Json<SearchResponse> {
    // Bound the forwarded query too, not just the local database lookup.
    let query = SearchQuery {
        q: query.q.chars().take(200).collect(),
    };
    let mut response = local_answer(&state, &query.q, false).await;
    if state.relay.clients() > 0 {
        let have: HashSet<String> = response.entries.iter().map(|e| e.body.clone()).collect();
        for (info_hash, wire) in state.relay.forward_search(&state, &query.q).await {
            if response.entries.len() >= 2 * MAX_RESULTS as usize {
                break;
            }
            if !have.contains(&wire.body) {
                response.entries.push(wire);
                response.relayed.push(info_hash);
            }
        }
    }
    Json(response)
}

/// Collects addresses from a DHT lookup for `key` for at most `time`.
/// With `announce_port`, this node is also announced under the key.
pub async fn dht_lookup(
    state: &AppState,
    key: [u8; 20],
    announce_port: Option<u16>,
    time: Duration,
    max: usize,
) -> Vec<SocketAddr> {
    if time.is_zero() {
        return Vec::new();
    }
    let Some(dht) = state.torrents.dht() else {
        return Vec::new();
    };
    let peers = dht.get_peers(librqbit::dht::Id20::new(key), announce_port);
    collect_dht_peers(
        peers,
        time,
        state.config.ipv4_only,
        if announce_port.is_some() {
            None
        } else {
            Some(max)
        },
    )
    .await
}

// An announce must keep driving the lookup (dropping it cancels librqbit's
// workers), but the addresses it yields are unused. Never accumulate them.
async fn collect_dht_peers<S: futures_util::Stream<Item = SocketAddr> + Unpin>(
    mut peers: S,
    time: Duration,
    ipv4_only: bool,
    retain: Option<usize>,
) -> Vec<SocketAddr> {
    let cap = retain.unwrap_or(0).min(256);
    if retain.is_some() && cap == 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let deadline = tokio::time::Instant::now() + time;
    // Also bound duplicate/IPv6-only floods and cooperative CPU work. This
    // bounds our consumer, not every internal queue of the DHT dependency.
    for n in 0..4096 {
        if n % 64 == 0 {
            tokio::task::yield_now().await;
        }
        match tokio::time::timeout_at(deadline, peers.next()).await {
            Ok(Some(addr)) if retain.is_some() && (!ipv4_only || addr.is_ipv4()) => {
                let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
                if !public_discovery_address(addr) {
                    continue;
                }
                if seen.insert(addr) {
                    found.push(addr);
                }
                if found.len() >= cap {
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    found
}

/// Public discovery must never turn an untrusted DHT answer into a request
/// to loopback, the router, cloud metadata, or another private-network service.
/// Explicit peers and LAN discovery have separate, local sources of authority.
fn public_discovery_address(addr: SocketAddr) -> bool {
    if addr.port() < 1024 {
        return false;
    }
    match addr.ip().to_canonical() {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (18..=19).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 88 && c == 99))
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            // Native global unicast only. Exclude transition/translation
            // mechanisms that could embed a private IPv4 destination.
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && segments[0] != 0x3fff
        }
    }
}

/// Searches the network for `query`, storing what is found, and tells how
/// it went.
pub async fn search(state: &AppState, query: &str) -> SearchReport {
    let Ok(_turn) = state.searches.acquire().await else {
        return SearchReport::default();
    };
    let own = state.p2p_addr();
    let slots = tokio::sync::Semaphore::new(if state.torrents.gaming() {
        2
    } else {
        PARALLEL_SEARCH_REQUESTS
    });
    let db = state.db.clone();
    let q = query.to_string();
    let remembered = tokio::task::spawn_blocking(move || db.known_search_nodes(&q))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let configured: Vec<SocketAddr> = state
        .config
        .relays
        .iter()
        .copied()
        .chain(state.reach.relay())
        .chain(state.config.peers.iter().copied())
        .chain(state.lan.addresses().into_iter().take(MAX_NODES_PER_SEARCH))
        .chain(remembered)
        .filter(|n| Some(*n) != own && seen.insert(*n))
        .take(if state.config.dht {
            MAX_NODES_PER_SEARCH / 2
        } else {
            MAX_NODES_PER_SEARCH
        })
        .collect();

    // Configured nodes are asked right away, while the DHT finds others
    let from_dht = async {
        if !state.config.dht {
            return (DhtState::Off, Asked::default());
        }
        let dht = if wait_for_dht(state, Duration::from_secs(5)).await {
            DhtState::Ready
        } else if state.torrents.dht().is_some_and(|d| {
            let stats = d.stats();
            stats.routing_table_size + stats.routing_table_size_v6 > 0
        }) {
            // Still finding its way, but in contact
            DhtState::Ready
        } else {
            DhtState::NoContact
        };
        // Overflow routes share the existing lookup deadline and the previous
        // maximum of six concurrent exact/prefix lookups. They must not create
        // another full search round or recursively expand discovered peers.
        let deadline = Instant::now() + DHT_LOOKUP_TIME;
        let lookups = keywords::query_routing_keys(query)
            .into_iter()
            .enumerate()
            .map(|(index, word)| async move {
                let found = dht_lookup(
                    state,
                    keywords::dht_key(&word),
                    None,
                    deadline.saturating_duration_since(Instant::now()),
                    MAX_NODES_PER_SEARCH,
                )
                .await;
                (index, found)
            });
        let mut routes: Vec<_> = stream::iter(lookups)
            .buffer_unordered(2 * keywords::MAX_QUERY_WORDS)
            .collect()
            .await;
        routes.sort_unstable_by_key(|(index, _)| *index);
        let routes = routes.into_iter().map(|(_, found)| found).collect();
        let remaining = MAX_NODES_PER_SEARCH.saturating_sub(configured.len());
        let nodes = search_candidates(routes, &configured, own, remaining);
        let asked = ask_all(state, nodes.into_iter(), query, &slots).await;
        (dht, asked)
    };
    let (direct, (dht, found)) = tokio::join!(
        ask_all(state, configured.iter().copied(), query, &slots),
        from_dht
    );

    let db = state.db.clone();
    let _ = tokio::task::spawn_blocking(move || db.prune_found()).await;

    let mut failures = direct.failures;
    for (failure, n) in found.failures {
        *failures.entry(failure).or_default() += n;
    }
    let report = SearchReport {
        dht,
        nodes_asked: direct.asked + found.asked,
        nodes_answered: direct.answered + found.answered,
        failure: failures
            .into_iter()
            .max_by_key(|&(_, n)| n)
            .map(|(failure, _)| failure),
        timed_out: direct.timed_out || found.timed_out,
    };
    tracing::debug!("Searched the network for {query:?}: {report:?}");
    report
}

/// Fairly interleave independent routes within the existing request budget.
/// A broad prefix/group cannot consume every slot before an exact route.
fn search_candidates(
    routes: Vec<Vec<SocketAddr>>,
    configured: &[SocketAddr],
    own: Option<SocketAddr>,
    limit: usize,
) -> Vec<SocketAddr> {
    let mut seen: HashSet<_> = configured.iter().copied().chain(own).collect();
    let mut routes: Vec<_> = routes.into_iter().map(Vec::into_iter).collect();
    let mut result = Vec::new();
    loop {
        let before = result.len();
        for route in &mut routes {
            if result.len() == limit {
                return result;
            }
            if let Some(node) = route.find(|node| seen.insert(*node)) {
                result.push(node);
            }
        }
        if result.len() == before {
            return result;
        }
    }
}

/// Recheck sources of the displayed results without launching new DHT searches.
pub async fn refresh_sources(state: &AppState, query: &str) {
    let Ok(_turn) = state.searches.acquire().await else {
        return;
    };
    let db = state.db.clone();
    let q = query.to_string();
    let nodes = tokio::task::spawn_blocking(move || db.known_search_nodes(&q))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let slots = tokio::sync::Semaphore::new(if state.torrents.gaming() {
        2
    } else {
        PARALLEL_SEARCH_REQUESTS
    });
    ask_all(
        state,
        nodes.into_iter().take(MAX_NODES_PER_SEARCH),
        query,
        &slots,
    )
    .await;
}

async fn forget_failed_source(state: &AppState, node: SocketAddr) {
    let db = state.db.clone();
    let _ = tokio::task::spawn_blocking(move || db.forget_source_node(node)).await;
}

/// What came of asking some nodes.
#[derive(Default)]
struct Asked {
    asked: u32,
    answered: u32,
    failures: std::collections::HashMap<SearchFailure, u32>,
    timed_out: bool,
}

/// Asks `nodes` to search for `query` and stores what they answer, each
/// answer as it arrives.
async fn ask_all(
    state: &AppState,
    nodes: impl Iterator<Item = SocketAddr>,
    query: &str,
    slots: &tokio::sync::Semaphore,
) -> Asked {
    let deadline = tokio::time::Instant::now() + state.config.search_timeout;
    let nodes: Vec<SocketAddr> = nodes.take(MAX_NODES_PER_SEARCH).collect();
    let mut pending: HashSet<_> = nodes.iter().copied().collect();
    let mut asked = Asked {
        asked: nodes.len() as u32,
        ..Asked::default()
    };
    let mut answers = stream::iter(nodes)
        .map(|node| async move {
            let _slot = slots.acquire().await.expect("search budget stays open");
            (node, ask(state, node, query).await)
        })
        .buffer_unordered(PARALLEL_SEARCH_REQUESTS);
    loop {
        let (node, answer) = match tokio::time::timeout_at(deadline, answers.next()).await {
            Ok(Some(next)) => next,
            Ok(None) => break,
            Err(_) => {
                tracing::debug!("Network search timed out; keeping the answers so far");
                asked.timed_out = true;
                break;
            }
        };
        pending.remove(&node);
        match answer {
            Ok(response) => {
                if response.node == state.identity.public_hex() {
                    asked.asked -= 1;
                    continue;
                }
                asked.answered += 1;
                let db = state.db.clone();
                let q = query.to_string();
                let _ =
                    tokio::task::spawn_blocking(move || db.forget_query_sources(node, &q)).await;
                store_answer(state, node, response).await;
            }
            Err(err) => {
                tracing::debug!("No answer from {node}: {err:#}");
                forget_failed_source(state, node).await;
                *asked.failures.entry(classify(&err)).or_default() += 1;
            }
        }
    }
    for node in pending {
        forget_failed_source(state, node).await;
    }
    asked
}

/// Why a node didn't answer a search.
fn classify(err: &anyhow::Error) -> SearchFailure {
    match err.downcast_ref::<reqwest::Error>() {
        Some(e) if e.is_timeout() => SearchFailure::Timeout,
        Some(e) if e.is_connect() => SearchFailure::Unreachable,
        Some(e) if e.is_request() => SearchFailure::Unreachable,
        _ => SearchFailure::BadAnswer,
    }
}

async fn ask(state: &AppState, node: SocketAddr, query: &str) -> anyhow::Result<SearchResponse> {
    let url = format!(
        "http://{node}{SEARCH_PATH}?q={}",
        percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
    );
    let response = state
        .http
        .get(url)
        .timeout(state.config.search_timeout)
        .send()
        .await?;
    let answer: SearchResponse = read_json(response, MAX_RESPONSE_BYTES).await?;
    anyhow::ensure!(
        answer.entries.len() <= 2 * MAX_RESULTS as usize
            && answer.relayed.len() <= 2 * MAX_RESULTS as usize,
        "too many search results"
    );
    Ok(answer)
}

async fn store_answer(state: &AppState, node: SocketAddr, response: SearchResponse) {
    let source = response
        .bt_port
        .filter(|&p| p != 0)
        .map(|port| SocketAddr::new(node.ip().to_canonical(), port));
    let relayed: HashSet<String> = response.relayed.into_iter().collect();
    let limits = state.config.limits.clone();
    let db = state.db.clone();
    let now = now();
    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        for wire in response.entries.iter().take(2 * MAX_RESULTS as usize) {
            let entry = match Entry::verify(wire, &limits, now) {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::debug!("Dropping an entry from {node}: {err}");
                    continue;
                }
            };
            let id = db.insert_entry(&entry, TrackState::Found)?;
            if let Some(source) = source {
                // A relayed track is downloaded from the relay, once it has
                // fetched it from its client
                let relay = relayed.contains(&entry.info_hash).then_some(node);
                db.add_source(
                    id,
                    Source {
                        addr: source,
                        relay,
                        node: Some(node),
                    },
                )?;
            }
        }
        Ok(())
    })
    .await;
    if let Ok(Err(err)) = result {
        tracing::warn!("Unable to store search results: {err}");
    }
}

/// Keeps this node findable: announces the words of the tracks it has (if
/// others can reach it), those of its relay clients, and itself as a node,
/// periodically and whenever `state.announce` is notified.
pub async fn announce_loop(state: AppState) {
    if state.torrents.dht().is_none() {
        return;
    }
    let mut plan = AnnouncePlan::default();
    // Spread simultaneous starts and coalesce the initial library scan.
    tokio::time::sleep(Duration::from_secs(5) + announce_jitter() / 12).await;
    loop {
        wait_for_dht(&state, Duration::from_secs(60)).await;
        if let Some(port) = state.p2p_addr().map(|a| a.port()) {
            let direct = state.reach.direct();
            let mut words = Vec::new();
            if direct {
                let db = state.db.clone();
                let texts = tokio::task::spawn_blocking(move || db.held_texts())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                words = keywords::announced_keys(texts.iter().map(|t| {
                    [
                        t[0].as_deref(),
                        t[1].as_deref(),
                        t[2].as_deref(),
                        t[3].as_deref(),
                    ]
                }));
                let mut seen: HashSet<String> = words.iter().cloned().collect();
                words.extend(
                    state
                        .relay
                        .relayed_words()
                        .into_iter()
                        .filter(|w| seen.insert(w.clone())),
                );
            }
            tracing::debug!("Announcing {} words", words.len());
            let mut keys: Vec<[u8; 20]> = words.iter().map(|w| keywords::dht_key(w)).collect();
            if direct {
                keys.insert(0, reach::nodes_key());
            }
            let now = Instant::now();
            plan.sync((port, direct), &keys, now);
            let due = plan.due(now, 64);
            let mut work = stream::iter(due)
                .map(|key| {
                    let state = &state;
                    async move {
                        dht_lookup(state, key, Some(port), ANNOUNCE_LOOKUP_TIME, 0).await;
                        key
                    }
                })
                .buffer_unordered(PARALLEL_ANNOUNCES);
            while let Some(key) = work.next().await {
                plan.completed(key, Instant::now(), announce_jitter());
            }
        }
        let interval = plan.wait(Instant::now());
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = state.announce.notified() => {
                // Debounce library changes without slowing already-due batches.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct InfoQuery {
    info_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InfoResponse {
    /// Base64 of the info dictionary
    pub info: String,
}

/// Sends the info dictionary of a track this node knows it of. It is
/// checked by the receiver against the signed entry, so it can come from
/// anyone.
async fn info_handler(
    State(state): State<AppState>,
    Query(query): Query<InfoQuery>,
) -> Result<Json<InfoResponse>, axum::http::StatusCode> {
    if !crate::torrent_info::is_info_hash(&query.info_hash) {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    let db = state.db.clone();
    let info = tokio::task::spawn_blocking(move || db.info_for(&query.info_hash))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    Ok(Json(InfoResponse {
        info: STANDARD.encode(info),
    }))
}

/// Gets a track's info dictionary from one of the nodes that answered the
/// search, checks it against the signed entry and stores it.
#[derive(Debug, thiserror::Error)]
#[error("No available source could provide this track")]
pub struct TrackUnavailable;

pub async fn fetch_info(
    state: &AppState,
    id: i64,
    nodes: &[SocketAddr],
) -> anyhow::Result<Vec<u8>> {
    let db = state.db.clone();
    let (body, sig) = tokio::task::spawn_blocking(move || db.entry_bytes(id))
        .await??
        .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
    let entry = Entry::from_stored(body, sig, None, &state.config.limits, now())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for node in nodes {
        let url = format!("http://{node}{INFO_PATH}?info_hash={}", entry.info_hash);
        let answer = async {
            let response = state
                .http
                .get(url)
                .timeout(Duration::from_secs(10))
                .send()
                .await?;
            let answer: InfoResponse =
                read_json(response, crate::entry::MAX_INFO_BYTES * 2).await?;
            anyhow::Ok(STANDARD.decode(answer.info)?)
        };
        let info = match answer.await {
            Ok(info) => info,
            Err(err) => {
                tracing::debug!("No torrent from {node}: {err:#}");
                let db = state.db.clone();
                let node = *node;
                let _ = tokio::task::spawn_blocking(move || db.forget_track_source(id, node)).await;
                continue;
            }
        };
        let mut checked = entry.clone();
        if let Err(err) = checked.attach_info(info.clone(), &state.config.limits) {
            tracing::info!("{node} sent a torrent that doesn't match: {err}");
            let db = state.db.clone();
            let node = *node;
            let _ = tokio::task::spawn_blocking(move || db.forget_track_source(id, node)).await;
            continue;
        }
        let db = state.db.clone();
        let stored = info.clone();
        tokio::task::spawn_blocking(move || db.set_info(id, &stored)).await??;
        return Ok(info);
    }
    Err(TrackUnavailable.into())
}

/// Waits, at most `timeout`, until the DHT knows enough nodes to look
/// anything up. Right after the node starts it knows none, and lookups and
/// announces made then reach nobody. Returns whether it is ready.
pub async fn wait_for_dht(state: &AppState, timeout: Duration) -> bool {
    let Some(dht) = state.torrents.dht() else {
        return false;
    };
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let stats = dht.stats();
        if stats.routing_table_size + stats.routing_table_size_v6 >= DHT_READY_NODES {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!("The DHT is not ready yet");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Sends the cover art of a track this node has it of, exactly as it got
/// it: the receiver checks it against the hash in the signed entry.
async fn cover_handler(
    State(state): State<AppState>,
    Query(query): Query<InfoQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], Vec<u8>), axum::http::StatusCode> {
    if !crate::torrent_info::is_info_hash(&query.info_hash) {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    let db = state.db.clone();
    let hash = query.info_hash.clone();
    let here = tokio::task::spawn_blocking(move || db.has_cover_for(&hash))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false);
    if !here {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }
    let data = tokio::fs::read(state.storage.cover_path(&query.info_hash))
        .await
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg")], data))
}

/// Per-key deadlines prevent a large import from re-announcing every old word.
/// Identity/port changes invalidate the schedule; wire keys stay compatible.
#[derive(Default)]
struct AnnouncePlan {
    address: Option<(u16, bool)>,
    pending: HashMap<[u8; 20], (Instant, usize)>,
}
impl AnnouncePlan {
    fn sync(&mut self, address: (u16, bool), keys: &[[u8; 20]], now: Instant) {
        if self.address != Some(address) {
            self.pending.clear();
            self.address = Some(address);
        }
        let wanted: HashSet<_> = keys.iter().copied().collect();
        self.pending.retain(|key, _| wanted.contains(key));
        for key in keys {
            self.pending.entry(*key).or_insert((now, 0));
        }
    }
    fn due(&self, now: Instant, limit: usize) -> Vec<[u8; 20]> {
        let mut keys: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .collect();
        keys.sort_unstable_by_key(|(key, (at, _))| (*at, **key));
        keys.into_iter().take(limit).map(|(key, _)| *key).collect()
    }
    fn completed(&mut self, key: [u8; 20], now: Instant, jitter: Duration) {
        if let Some((at, round)) = self.pending.get_mut(&key) {
            *at = now
                + EARLY_ANNOUNCE_INTERVALS
                    .get(*round)
                    .copied()
                    .unwrap_or(ANNOUNCE_INTERVAL)
                + jitter;
            *round = round.saturating_add(1).min(EARLY_ANNOUNCE_INTERVALS.len());
        }
    }
    fn wait(&self, now: Instant) -> Duration {
        self.pending
            .values()
            .map(|(at, _)| at.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_secs(60))
            .min(Duration::from_secs(60))
    }
}
fn announce_jitter() -> Duration {
    let mut bytes = [0u8; 2];
    let _ = getrandom::fill(&mut bytes);
    Duration::from_millis(u64::from(u16::from_le_bytes(bytes)) % 60_000)
}

#[cfg(test)]
mod scaling_tests {
    use super::*;
    #[test]
    fn exact_and_overflow_routes_share_a_bounded_peer_budget() {
        let address = |n| SocketAddr::from(([127, 0, 0, 1], n));
        let exact = vec![address(1), address(2), address(3)];
        let overflow = (10..100).map(address).collect();
        let candidates =
            search_candidates(vec![exact, overflow], &[address(1)], Some(address(10)), 4);
        assert_eq!(
            candidates,
            vec![address(2), address(11), address(3), address(12)]
        );
        assert!(search_candidates(vec![vec![address(1)]], &[], None, 0).is_empty());
        assert_eq!(
            search_candidates(
                vec![vec![address(1); 100], vec![address(1), address(2)]],
                &[],
                None,
                40
            ),
            vec![address(1), address(2)]
        );
    }

    #[tokio::test]
    async fn dht_duplicates_floods_and_announcements_use_bounded_memory() {
        let peer = "8.8.8.8:47391".parse().unwrap();
        let duplicate_flood = stream::repeat(peer);
        assert_eq!(
            collect_dht_peers(
                duplicate_flood,
                Duration::from_secs(1),
                false,
                Some(usize::MAX)
            )
            .await,
            vec![peer]
        );
        let many = stream::iter((1024..=6024).map(|port| SocketAddr::from(([8, 8, 8, 8], port))));
        assert_eq!(
            collect_dht_peers(many, Duration::from_secs(1), false, Some(usize::MAX))
                .await
                .len(),
            256
        );
        assert!(
            collect_dht_peers(stream::repeat(peer), Duration::from_secs(1), false, None)
                .await
                .is_empty()
        );
    }
    #[tokio::test]
    async fn public_discovery_rejects_internal_and_embedded_addresses() {
        let blocked = [
            "127.0.0.1:47391",
            "10.0.0.1:47391",
            "192.168.1.1:47391",
            "172.16.0.1:47391",
            "169.254.169.254:47391",
            "100.64.0.1:47391",
            "198.18.0.1:47391",
            "0.0.0.0:47391",
            "255.255.255.255:47391",
            "224.0.0.1:47391",
            "192.0.2.1:47391",
            "8.8.8.8:80",
            "[::1]:47391",
            "[::ffff:192.168.1.1]:47391",
            "[fc00::1]:47391",
            "[fe80::1]:47391",
            "[ff02::1]:47391",
            "[64:ff9b::c0a8:101]:47391",
            "[2002:c0a8:101::1]:47391",
            "[2001::1]:47391",
            "[2001:db8::1]:47391",
        ];
        let public = ["8.8.8.8:47391", "[2606:4700:4700::1111]:47391"];
        let addresses = blocked.iter().chain(&public).map(|s| s.parse().unwrap());
        assert_eq!(
            collect_dht_peers(
                stream::iter(addresses),
                Duration::from_secs(1),
                false,
                Some(50)
            )
            .await,
            public
                .iter()
                .map(|s| s.parse::<SocketAddr>().unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn importing_one_new_key_does_not_reannounce_the_old_keys() {
        let mut plan = AnnouncePlan::default();
        let now = Instant::now();
        let old = [1; 20];
        let new = [2; 20];
        plan.sync((47391, true), &[old], now);
        plan.completed(old, now, Duration::ZERO);
        for n in 1..100 {
            plan.sync((47391, true), &[old], now + Duration::from_millis(n));
        }
        assert!(plan.due(now + Duration::from_secs(1), 32).is_empty());
        plan.sync((47391, true), &[old, new], now + Duration::from_secs(1));
        assert_eq!(plan.due(now + Duration::from_secs(1), 32), vec![new]);
        plan.sync((47392, true), &[old, new], now);
        assert_eq!(plan.due(now, 32).len(), 2);
        plan.sync((47392, false), &[], now);
        assert!(plan.pending.is_empty());
    }
    #[test]
    fn announce_batches_are_bounded_and_periodic_refresh_is_retained() {
        let mut plan = AnnouncePlan::default();
        let now = Instant::now();
        let keys: Vec<_> = (0u16..1401)
            .map(|n| {
                let mut k = [0; 20];
                k[..2].copy_from_slice(&n.to_be_bytes());
                k
            })
            .collect();
        plan.sync((47391, true), &keys, now);
        let batch = plan.due(now, 32);
        assert_eq!(batch.len(), 32);
        for k in &batch {
            plan.completed(*k, now, Duration::ZERO);
        }
        assert!(!plan.due(now, 32).contains(&batch[0]));
        plan.sync((47391, true), &[batch[0]], now);
        assert_eq!(plan.due(now + Duration::from_secs(120), 32), vec![batch[0]]);
    }
}
