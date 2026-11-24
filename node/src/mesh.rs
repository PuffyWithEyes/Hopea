// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded search forwarding over user-operated peers. No request supplies a
//! destination: forwarding only uses locally configured/discovered neighbors.

use crate::{
    db,
    entry::{Entry, WireEntry},
    p2p::{self, SearchResponse},
    reach, relay, AppState,
};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, sync::Semaphore};

pub const MEDIA_PATH: &str = "/p2p/v1/media";
const MAX_HOPS: u8 = 2;
const MAX_SEEN: usize = 512;
const MAX_OFFERS: usize = 512;
const MAX_ENTRIES: usize = 50;
const MAX_WIRE: usize = 768 * 1024;
const REQUEST_TTL: i64 = 30;
const OFFER_TTL: Duration = Duration::from_secs(10 * 60);
const FORWARD_TIME: Duration = Duration::from_secs(6);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    id: String,
    node: String,
    issued: i64,
    hops: u8,
    signature: String,
}
impl Request {
    pub(crate) fn new(state: &AppState, query: &str) -> Self {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).expect("OS randomness unavailable");
        let mut request = Self {
            id: hex::encode(nonce),
            node: state.identity.public_hex(),
            issued: db::now(),
            hops: MAX_HOPS,
            signature: String::new(),
        };
        request.signature = hex::encode(state.identity.sign(&request.bytes(query)));
        state.mesh.admit(&request);
        request
    }
    fn bytes(&self, query: &str) -> Vec<u8> {
        let mut bytes = b"hopea-mesh-search-v1\0".to_vec();
        bytes.extend(
            serde_json::to_vec(&(&self.id, &self.node, self.issued, query, MAX_HOPS)).unwrap(),
        );
        bytes
    }
    fn valid(&self, query: &str) -> bool {
        if self.id.len() != 32
            || !self.id.bytes().all(|b| b.is_ascii_hexdigit())
            || self.node.len() != 64
            || self.signature.len() != 128
            || self.hops > MAX_HOPS
            || query.chars().count() > 200
            || self.issued.abs_diff(db::now()) > REQUEST_TTL as u64
        {
            return false;
        }
        let Ok(key) = hex::decode(&self.node) else {
            return false;
        };
        let Ok(key): Result<[u8; 32], _> = key.try_into() else {
            return false;
        };
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&key) else {
            return false;
        };
        let Ok(sig) = hex::decode(&self.signature) else {
            return false;
        };
        let Ok(sig) = ed25519_dalek::Signature::from_slice(&sig) else {
            return false;
        };
        key.verify_strict(&self.bytes(query), &sig).is_ok()
    }
    pub(crate) fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap())
    }
    pub(crate) fn decode(value: &str, query: &str) -> Option<Self> {
        if value.len() > 1024 {
            return None;
        }
        let request: Self = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(value).ok()?).ok()?;
        request.valid(query).then_some(request)
    }
}

pub(crate) struct Mesh {
    seen: Mutex<HashMap<(String, String), Instant>>,
    offers: Mutex<HashMap<String, (SocketAddr, Instant)>>,
    forwarding: Semaphore,
    rate: Mutex<(Instant, f64)>,
    round: AtomicUsize,
    pub changed: tokio::sync::Notify,
    sending: std::sync::Arc<Semaphore>,
    fetching: Semaphore,
    fetching_chain: Semaphore,
    starts: [std::sync::Arc<tokio::sync::Mutex<()>>; 64],
}
impl Default for Mesh {
    fn default() -> Self {
        Self {
            seen: Mutex::default(),
            offers: Mutex::default(),
            forwarding: Semaphore::new(2),
            rate: Mutex::new((Instant::now(), 2.0)),
            round: AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
            sending: std::sync::Arc::new(Semaphore::new(2)),
            fetching: Semaphore::new(2),
            fetching_chain: Semaphore::new(2),
            starts: std::array::from_fn(|_| std::sync::Arc::new(tokio::sync::Mutex::new(()))),
        }
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
impl Mesh {
    /// Fixed-size striped locks serialize starts/writes for the same track
    /// without retaining a map controlled by remote hashes.
    pub(crate) async fn lock_track(
        &self,
        hash: &str,
    ) -> std::sync::Arc<tokio::sync::OwnedMutexGuard<()>> {
        let bucket = hash
            .bytes()
            .fold(0usize, |n, b| n.wrapping_mul(31).wrapping_add(b as usize))
            % self.starts.len();
        std::sync::Arc::new(self.starts[bucket].clone().lock_owned().await)
    }
    fn admit(&self, request: &Request) -> bool {
        let now = Instant::now();
        let mut seen = lock(&self.seen);
        seen.retain(|_, at| now.duration_since(*at) < Duration::from_secs(2 * REQUEST_TTL as u64));
        let key = (request.node.clone(), request.id.clone());
        if seen.contains_key(&key) || seen.len() >= MAX_SEEN {
            return false;
        }
        seen.insert(key, now);
        true
    }
    fn forward_allowed(&self) -> bool {
        let now = Instant::now();
        let mut rate = lock(&self.rate);
        rate.1 = (rate.1 + now.duration_since(rate.0).as_secs_f64()).min(2.0);
        rate.0 = now;
        if rate.1 < 1.0 {
            return false;
        }
        rate.1 -= 1.0;
        true
    }
    fn offer(&self, hash: String, peer: SocketAddr) -> bool {
        let mut offers = lock(&self.offers);
        offers.retain(|_, (_, at)| at.elapsed() < OFFER_TTL);
        if offers.len() >= MAX_OFFERS && !offers.contains_key(&hash) {
            return false;
        }
        offers.insert(hash, (peer, Instant::now()));
        true
    }
    pub(crate) fn source(&self, hash: &str) -> Option<SocketAddr> {
        lock(&self.offers)
            .get(hash)
            .filter(|(_, at)| at.elapsed() < OFFER_TTL)
            .map(|(peer, _)| *peer)
    }
}

/// No DHT lookups or peer-list expansion in response to a forwarded query.
async fn neighbors(state: &AppState, limit: usize) -> Vec<SocketAddr> {
    let db = state.db.clone();
    let ipv4 = state.config.ipv4_only;
    let remembered = tokio::task::spawn_blocking(move || db.known_peers(ipv4))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let explicit = state
        .config
        .peers
        .iter()
        .chain(&state.config.relays)
        .copied()
        .chain(state.reach.relay())
        .chain(state.relay.outgoing_peers())
        .collect();
    let lan = if ipv4 {
        Vec::new()
    } else {
        state.lan.addresses()
    };
    p2p::rotating_candidates(
        vec![explicit, lan, remembered],
        &[],
        state.p2p_addr(),
        limit,
        state.mesh.round.fetch_add(1, Ordering::Relaxed),
    )
}

pub(crate) async fn search(state: &AppState, query: &str, request: Request) -> SearchResponse {
    let mut answer = SearchResponse {
        node: state.identity.public_hex(),
        bt_port: state.torrents.bt_port(),
        entries: Vec::new(),
        relayed: Vec::new(),
    };
    if !state.mesh.admit(&request) {
        return answer;
    }
    let mut have = HashSet::new();
    let mut bytes = 0usize;
    // A direct result retains the legacy lazy info fetch (and live-source
    // check). Intermediaries need info to validate and retain a forwarding offer.
    let local = p2p::local_answer(state, query, request.hops < MAX_HOPS).await;
    for wire in local.entries {
        push(&mut answer, &mut have, &mut bytes, wire, None);
    }
    let relay = relay_entries(state, query);
    let forwarded = forwarded_entries(state, query, request);
    let (relay, forwarded) = tokio::join!(relay, forwarded);
    for (hash, wire) in relay.into_iter().chain(forwarded) {
        push(&mut answer, &mut have, &mut bytes, wire, Some(hash));
    }
    answer
}

async fn relay_entries(state: &AppState, query: &str) -> Vec<(String, WireEntry)> {
    let mut entries = Vec::new();
    // Reverse connections to NATed users retain the existing signed relay
    // registration, task limits and validation. Include their verified info.
    for (hash, _) in state.relay.forward_search(state, query).await {
        let db = state.db.clone();
        let wanted = hash.clone();
        let wire = tokio::task::spawn_blocking(move || {
            let id = db.id_for_info_hash(&wanted).ok()??;
            let (body, sig) = db.entry_bytes(id).ok()??;
            let info = db.info_for(&wanted).ok()??;
            Some(WireEntry {
                body: base64::engine::general_purpose::STANDARD.encode(body),
                sig: base64::engine::general_purpose::STANDARD.encode(sig),
                info: Some(base64::engine::general_purpose::STANDARD.encode(info)),
            })
        })
        .await
        .ok()
        .flatten();
        if let Some(wire) = wire {
            entries.push((hash, wire));
        }
    }
    entries
}

async fn forwarded_entries(
    state: &AppState,
    query: &str,
    request: Request,
) -> Vec<(String, WireEntry)> {
    if request.hops == 0 || !state.config.relay || !state.mesh.forward_allowed() {
        return Vec::new();
    }
    let Ok(_slot) = state.mesh.forwarding.try_acquire() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut have = HashSet::new();
    let mut child = request;
    child.hops -= 1;
    let peers = neighbors(state, if state.torrents.gaming() { 1 } else { 2 }).await;
    let replies = stream::iter(peers)
        .map(|peer| {
            let child = child.clone();
            async move {
                let url = format!(
                    "http://{peer}{}?q={}&mesh={}",
                    p2p::SEARCH_PATH,
                    percent_encoding::utf8_percent_encode(
                        query,
                        percent_encoding::NON_ALPHANUMERIC
                    ),
                    child.encode()
                );
                let reply = async {
                    let response = state.http.get(url).timeout(FORWARD_TIME).send().await?;
                    reach::read_json::<SearchResponse>(response, MAX_WIRE + 128 * 1024).await
                }
                .await;
                (peer, reply)
            }
        })
        .buffer_unordered(2);
    tokio::pin!(replies);
    while let Some((peer, reply)) = replies.next().await {
        let Ok(reply) = reply else {
            continue;
        };
        let limits = state.config.limits.clone();
        let db = state.db.clone();
        let verified = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for wire in reply.entries.into_iter().take(MAX_ENTRIES) {
                if let Ok(entry) = Entry::verify(&wire, &limits, db::now()) {
                    if entry.info_bytes.is_some()
                        && db.insert_entry(&entry, db::State::Found).is_ok()
                    {
                        out.push((entry.info_hash, wire));
                    }
                }
            }
            let _ = db.prune_found();
            out
        })
        .await
        .unwrap_or_default();
        for (hash, wire) in verified {
            if have.insert(hash.clone()) && state.mesh.offer(hash.clone(), peer) {
                entries.push((hash, wire));
            }
        }
    }
    entries
}
fn push(
    answer: &mut SearchResponse,
    have: &mut HashSet<String>,
    bytes: &mut usize,
    wire: WireEntry,
    relayed: Option<String>,
) {
    let size = wire.body.len() + wire.sig.len() + wire.info.as_ref().map_or(0, String::len) + 256;
    if answer.entries.len() >= MAX_ENTRIES
        || bytes.saturating_add(size) > MAX_WIRE
        || !have.insert(wire.body.clone())
    {
        return;
    }
    *bytes += size;
    answer.entries.push(wire);
    if let Some(hash) = relayed {
        answer.relayed.push(hash);
    }
}

#[derive(Deserialize)]
pub(crate) struct MediaQuery {
    info_hash: String,
}

/// Serves only already validated audio. GET never fetches an arbitrary target
/// or starts a relay chain. The permit lives until the response body is dropped.
pub(crate) async fn media(
    State(state): State<AppState>,
    Query(query): Query<MediaQuery>,
) -> Response {
    if !crate::torrent_info::is_info_hash(&query.info_hash) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(permit) = state.mesh.sending.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let db = state.db.clone();
    let hash = query.info_hash;
    let row = tokio::task::spawn_blocking(move || {
        db.id_for_info_hash(&hash)
            .ok()
            .flatten()
            .and_then(|id| db.torrent(id).ok().flatten())
    })
    .await
    .ok()
    .flatten();
    let Some(row) = row.filter(|row| row.state.has_file()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(file) =
        tokio::fs::File::open(state.storage.media_path(&row.info_hash, row.format)).await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(metadata) = file.metadata().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let length = metadata.len();
    let stream = stream::try_unfold(
        (file, permit, state, Instant::now()),
        |(mut file, permit, state, started)| async move {
            if started.elapsed() > Duration::from_secs(150) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "audio transfer deadline",
                ));
            }
            let mut bytes = vec![0u8; 16 * 1024];
            let count = file.read(&mut bytes).await?;
            if count == 0 {
                return Ok(None);
            }
            bytes.truncate(count);
            // Share the real BitTorrent limiter, including user and Game Mode
            // changes, rather than granting each transport an independent quota.
            tokio::time::timeout(
                Duration::from_secs(150).saturating_sub(started.elapsed()),
                state.torrents.charge_http(count, true),
            )
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)?;
            // Also cap otherwise-unlimited HTTP traffic at 1 MiB/s per stream.
            tokio::time::sleep(Duration::from_secs_f64(count as f64 / (1024 * 1024) as f64)).await;
            Ok(Some((bytes, (file, permit, state, started))))
        },
    );
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

pub(crate) async fn download(state: &AppState, peer: SocketAddr, hash: &str) -> anyhow::Result<()> {
    let _track = tokio::time::timeout(Duration::from_secs(5), state.mesh.lock_track(hash)).await?;
    download_locked(state, peer, hash, _track).await
}

/// Caller holds the track's start lock, also used by BitTorrent and uploads.
pub(crate) async fn download_locked(
    state: &AppState,
    peer: SocketAddr,
    hash: &str,
    track: std::sync::Arc<tokio::sync::OwnedMutexGuard<()>>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::torrent_info::is_info_hash(hash),
        "invalid audio hash"
    );
    let _slot = state
        .mesh
        .fetching
        .try_acquire()
        .map_err(|_| anyhow::anyhow!("HTTP downloads busy"))?;
    let response = state
        .http
        .get(format!("http://{peer}{MEDIA_PATH}?info_hash={hash}"))
        .timeout(Duration::from_secs(150))
        .send()
        .await?
        .error_for_status()?;
    let torrents = state.torrents.clone();
    let stream = stream::try_unfold(
        (response, torrents),
        |(mut response, torrents)| async move {
            let Some(chunk) = response.chunk().await? else {
                return anyhow::Ok(None);
            };
            torrents.charge_http(chunk.len(), false).await?;
            Ok(Some((chunk, (response, torrents))))
        },
    );
    tokio::time::timeout(
        Duration::from_secs(180),
        relay::receive_upload_locked(state, hash, Body::from_stream(stream), track),
    )
    .await?
}

pub(crate) async fn fetch(state: &AppState, hash: &str, hops: u8) -> anyhow::Result<()> {
    anyhow::ensure!(hops > 0 && hops <= MAX_HOPS + 1, "relay path exhausted");
    let _slot = state
        .mesh
        .fetching_chain
        .try_acquire()
        .map_err(|_| anyhow::anyhow!("relay downloads busy"))?;
    let peer = state
        .mesh
        .source(hash)
        .ok_or_else(|| anyhow::anyhow!("no live mesh offer"))?;
    let response = state
        .http
        .post(format!(
            "http://{peer}{}?info_hash={hash}&hops={}",
            relay::FETCH_PATH,
            hops - 1
        ))
        .timeout(relay::FETCH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    drop(response);
    download(state, peer, hash).await
}

/// Periodically re-probe a small rotating set even after connectivity failures.
/// No loop depends on a successful previous round or requires an app restart.
pub(crate) async fn maintain(state: AppState) {
    let mut was_connected = false;
    loop {
        let peers = neighbors(&state, 4).await;
        let mut replies = stream::iter(peers)
            .map(|peer| {
                let state = &state;
                async move { reach::fetch_hello(state, peer).await.is_ok() }
            })
            .buffer_unordered(2);
        let mut connected = false;
        while let Some(ok) = replies.next().await {
            connected |= ok;
        }
        if connected != was_connected {
            state.mesh.changed.notify_one();
        }
        was_connected = connected;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signatures_replay_expiry_and_resource_limits() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::config::Config::isolated(dir.path().join("node")),
            &crate::auth::generate_token(),
        )
        .await
        .unwrap();
        let request = Request::new(&state, "night drive");
        assert!(Request::decode(&request.encode(), "night drive").is_some());
        assert!(Request::decode(&request.encode(), "different query").is_none());
        assert!(Request::decode(&"A".repeat(1025), "night drive").is_none());
        assert!(
            !state.mesh.admit(&request),
            "origin rejects its own looped query"
        );
        let mesh = Mesh::default();
        assert!(mesh.admit(&request));
        assert!(!mesh.admit(&request));
        let mut altered = request.clone();
        altered.hops = MAX_HOPS + 1;
        assert!(!altered.valid("night drive"));
        altered.hops = 0;
        assert!(altered.valid("night drive"));
        altered.issued -= 2 * REQUEST_TTL;
        altered.signature = hex::encode(state.identity.sign(&altered.bytes("night drive")));
        assert!(!altered.valid("night drive"));
        for index in 1..MAX_SEEN {
            altered.id = format!("{index:032x}");
            assert!(mesh.admit(&altered));
        }
        altered.id = "z".repeat(32);
        assert!(!mesh.admit(&altered));
        assert!(mesh.forward_allowed());
        assert!(mesh.forward_allowed());
        assert!(!mesh.forward_allowed());
        let peer = "127.0.0.1:12345".parse().unwrap();
        for index in 0..MAX_OFFERS {
            assert!(mesh.offer(format!("{index:040x}"), peer));
        }
        assert!(!mesh.offer("f".repeat(40), peer));
        let hash = "0".repeat(40);
        assert_eq!(mesh.source(&hash), Some(peer));
        lock(&mesh.offers).get_mut(&hash).unwrap().1 = Instant::now() - OFFER_TTL;
        assert_eq!(mesh.source(&hash), None);
        assert!(fetch(&state, &hash, 0).await.is_err());
        assert_eq!(
            media(
                State(state.clone()),
                Query(MediaQuery {
                    info_hash: "../etc/passwd".into()
                })
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            media(State(state), Query(MediaQuery { info_hash: hash }))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
