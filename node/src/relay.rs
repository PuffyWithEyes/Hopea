// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Relaying for nodes that nobody can connect to.
//!
//! A node that fails the reachability check (see [`crate::reach`]), for
//! example one behind carrier-grade NAT, registers with a reachable node
//! that offers to relay, and then keeps polling it. Both directions work
//! over connections the unreachable node opens itself:
//!
//! - **Being found.** The relay announces the client's words in the DHT
//!   under its own address. When a search reaches the relay, it forwards it
//!   to the clients that have one of the words, and returns their signed
//!   entries along with its own, marked as relayed.
//! - **Sending files.** A node that wants a relayed track asks the relay to
//!   fetch it ([`FETCH_PATH`]). The relay asks the client to upload it, runs
//!   the full validation on it like on any download, and then shares it over
//!   BitTorrent, from where the asking node streams it.
//!
//! A relay keeps what it fetched in its cache and shares it on, as any node
//! does with what it downloads. Everything a client sends is checked the
//! same way as anything else from the network.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use hopea_common::api::Reachability;
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, oneshot},
};

use crate::{
    db::{now, State as TrackState},
    entry::{Entry, WireEntry},
    ingest, keywords,
    p2p::SearchResponse,
    reach::{fetch_hello, read_json},
    torrent_info::{self, is_info_hash, InfoDict},
    AppState,
};

pub const REGISTER_PATH: &str = "/p2p/v1/relay/register";
pub const POLL_PATH: &str = "/p2p/v1/relay/poll";
pub const ANSWER_PATH: &str = "/p2p/v1/relay/answer";
pub const UPLOAD_PATH: &str = "/p2p/v1/relay/upload";
pub const METADATA_PATH: &str = "/p2p/v1/relay/metadata";
pub const LYRICS_PATH: &str = "/p2p/v1/relay/lyrics";
pub const FETCH_PATH: &str = "/p2p/v1/relay/fetch";

/// How long a poll waits for a task
const POLL_WAIT: Duration = Duration::from_secs(25);
/// A client that hasn't polled for this long is dropped
const SESSION_TTL: Duration = Duration::from_secs(70);
/// How long a relayed search waits for the clients
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a fetch waits for the client's upload and its validation
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a relayed entry can be fetched after it was returned
const OFFER_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_WORDS_PER_CLIENT: usize = keywords::MAX_ANNOUNCED_KEYS;
/// Words of all clients together that a relay announces at most
const MAX_RELAYED_WORDS: usize = 1000;
/// Clients send their words again this often, with any new tracks
const REREGISTER_INTERVAL: Duration = Duration::from_secs(10 * 60);
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;
const MAX_FORWARDED_CLIENTS: usize = 8;
const MAX_ENTRIES_PER_ANSWER: usize = 50;
const MAX_OFFERS: usize = 4096;
/// When no relay is found, how long to wait before looking again
const RETRY_INTERVAL: Duration = Duration::from_secs(2 * 60);
const MAX_RELAY_CANDIDATES: usize = 64;
const MAX_REGISTRATION_ATTEMPTS: usize = 4;
const MAX_CLIENT_TASKS: usize = 2;
// A hostile relay can reply immediately instead of long-polling. Do not let
// that turn into unlimited requests or background task creation.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Serialize, Deserialize)]
pub struct Register {
    pub node: String,
    pub words: Vec<String>,
    #[serde(default)]
    pub issued_at: i64,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub session: Option<String>,
}

impl Register {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = b"hopea-relay-register-v1\0".to_vec();
        bytes.extend(
            serde_json::to_vec(&(&self.node, &self.words, self.issued_at, &self.session))
                .expect("serializable registration"),
        );
        bytes
    }
    fn valid(&self, at: i64) -> bool {
        if self.node.len() != 64
            || self.signature.len() != 128
            || self.issued_at.abs_diff(at) > 60
            || self.words.len() > MAX_WORDS_PER_CLIENT
            || self.words.iter().any(|w| w.len() > 64)
            || self.session.as_ref().is_some_and(|s| s.len() != 32)
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
        let Ok(signature) = hex::decode(&self.signature) else {
            return false;
        };
        let Ok(signature) = ed25519_dalek::Signature::from_slice(&signature) else {
            return false;
        };
        key.verify_strict(&self.signing_bytes(), &signature).is_ok()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Registered {
    pub session: String,
}

/// Work a relay hands to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Task {
    /// Answer a search like `GET /p2p/v1/search` would
    Search { id: u64, q: String },
    /// Upload the file of this torrent
    Upload { info_hash: String },
}

struct Session {
    node: String,
    words: HashSet<String>,
    tx: mpsc::Sender<Task>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Task>>>,
    last_seen: Instant,
}

/// The relay side: clients, and what is waiting on them.
#[derive(Default)]
pub struct Server {
    sessions: Mutex<HashMap<String, Session>>,
    answers: Mutex<HashMap<u64, (String, oneshot::Sender<SearchResponse>)>>,
    /// Which client (by node key) returned each relayed track, and when
    offered: Mutex<HashMap<String, (String, Instant)>>,
    /// Fetches waiting for a track to be uploaded and validated
    waiting: Mutex<HashMap<String, Vec<oneshot::Sender<bool>>>>,
    next_id: AtomicU64,
    search_round: AtomicU64,
    outgoing: Outgoing,
}

struct Outgoing {
    peers: Mutex<std::collections::BTreeMap<SocketAddr, bool>>,
    tasks: Arc<tokio::sync::Semaphore>,
}
impl Default for Outgoing {
    fn default() -> Self {
        Self {
            peers: Mutex::default(),
            tasks: Arc::new(tokio::sync::Semaphore::new(MAX_CLIENT_TASKS)),
        }
    }
}

struct Connection<'a> {
    state: &'a AppState,
    peer: SocketAddr,
}
impl Drop for Connection<'_> {
    fn drop(&mut self) {
        let mut peers = lock(&self.state.relay.outgoing.peers);
        peers.remove(&self.peer);
        if let Some(peer) = peers
            .iter()
            .find_map(|(peer, active)| active.then_some(*peer))
        {
            self.state.reach.set(Reachability::Relay, Some(peer));
        } else {
            self.state.reach.set(Reachability::Unreachable, None);
        }
        self.state.announce.notify_one();
    }
}

struct PendingAnswers<'a> {
    server: &'a Server,
    ids: Vec<u64>,
}
impl Drop for PendingAnswers<'_> {
    fn drop(&mut self) {
        let mut answers = lock(&self.server.answers);
        for id in &self.ids {
            answers.remove(id);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    pub(crate) fn outgoing_peers(&self) -> Vec<SocketAddr> {
        lock(&self.outgoing.peers)
            .iter()
            .filter_map(|(peer, active)| active.then_some(*peer))
            .collect()
    }
    /// Whether this node takes new clients right now.
    pub fn accepting(&self, state: &AppState) -> bool {
        state.config.relay
            && state.reach.direct()
            && lock(&self.sessions).len() < state.config.max_relay_clients
    }

    /// The words this node announces for its clients.
    pub fn relayed_words(&self) -> Vec<String> {
        let sessions = lock(&self.sessions);
        keywords::bounded_routing_keys(
            sessions
                .values()
                .flat_map(|s| s.words.iter())
                .flat_map(|word| {
                    let group =
                        (!keywords::is_routing_group(word)).then(|| keywords::routing_group(word));
                    std::iter::once(word.clone()).chain(group)
                }),
            MAX_RELAYED_WORDS,
        )
    }

    pub fn clients(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Asks the clients that have any word of `query`, and returns their
    /// verified entries and the info hashes among them.
    pub async fn forward_search(&self, state: &AppState, query: &str) -> Vec<(String, WireEntry)> {
        let words: HashSet<String> = keywords::query_routing_keys(query).into_iter().collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut pending = Vec::new();
        let mut cleanup = PendingAnswers {
            server: self,
            ids: Vec::new(),
        };
        {
            let sessions = lock(&self.sessions);
            let exact: HashSet<_> = keywords::query_keys(query).into_iter().collect();
            let mut precise = Vec::new();
            let mut grouped = Vec::new();
            for (token, session) in sessions.iter() {
                if !session.words.is_disjoint(&exact) {
                    precise.push((token, session));
                } else if !session.words.is_disjoint(&words) {
                    grouped.push((token, session));
                }
            }
            let round = self.search_round.fetch_add(1, Ordering::Relaxed) as usize;
            for candidates in [&mut precise, &mut grouped] {
                candidates.sort_unstable_by(|a, b| a.0.cmp(b.0));
                if !candidates.is_empty() {
                    let offset = round % candidates.len();
                    candidates.rotate_left(offset);
                }
            }
            for (token, session) in precise
                .into_iter()
                .chain(grouped)
                .take(MAX_FORWARDED_CLIENTS)
            {
                let task_id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel();
                lock(&self.answers).insert(task_id, (token.clone(), tx));
                cleanup.ids.push(task_id);
                let task = Task::Search {
                    id: task_id,
                    q: query.to_string(),
                };
                if session.tx.try_send(task).is_ok() {
                    pending.push((session.node.clone(), task_id, rx));
                } else {
                    lock(&self.answers).remove(&task_id);
                }
            }
        }

        let answers = futures_util::future::join_all(pending.into_iter().map(
            |(node, task_id, rx)| async move {
                let answer = tokio::time::timeout(FORWARD_TIMEOUT, rx).await;
                lock(&self.answers).remove(&task_id);
                (node, answer.ok().and_then(Result::ok))
            },
        ))
        .await;

        let mut out = Vec::new();
        for (node, answer) in answers {
            let Some(answer) = answer else {
                continue;
            };
            let limits = state.config.limits.clone();
            let db = state.db.clone();
            let entries = answer.entries;
            let verified = tokio::task::spawn_blocking(move || {
                let now = now();
                entries
                    .into_iter()
                    .take(MAX_ENTRIES_PER_ANSWER)
                    .filter_map(|wire| {
                        let entry = Entry::verify(&wire, &limits, now).ok()?;
                        // Clients send their torrents along: the relay needs
                        // them to check what it fetches, and to hand them out
                        entry.info_bytes.as_ref()?;
                        db.insert_entry(&entry, TrackState::Found).ok()?;
                        Some((entry.info_hash.clone(), entry.to_wire(false)))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let mut offered = lock(&self.offered);
            for (info_hash, wire) in verified {
                if offered.len() >= MAX_OFFERS && !offered.contains_key(&info_hash) {
                    break;
                }
                offered.insert(info_hash.clone(), (node.clone(), Instant::now()));
                out.push((info_hash, wire));
            }
        }
        out
    }

    fn session_rx(&self, session: &str) -> Option<Arc<tokio::sync::Mutex<mpsc::Receiver<Task>>>> {
        let mut sessions = lock(&self.sessions);
        let s = sessions.get_mut(session)?;
        s.last_seen = Instant::now();
        Some(s.rx.clone())
    }

    fn resolve(&self, info_hash: &str, ok: bool) {
        if let Some(waiters) = lock(&self.waiting).remove(info_hash) {
            for w in waiters {
                let _ = w.send(ok);
            }
        }
    }
}

/// Drops clients that stopped polling and offers that are too old.
pub async fn maintain(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let server = &state.relay;
        let dropped = {
            let mut sessions = lock(&server.sessions);
            let before = sessions.len();
            sessions.retain(|_, s| s.last_seen.elapsed() < SESSION_TTL);
            before - sessions.len()
        };
        lock(&server.offered).retain(|_, (_, at)| at.elapsed() < OFFER_TTL);
        lock(&server.waiting).retain(|_, waiters| {
            waiters.retain(|w| !w.is_closed());
            !waiters.is_empty()
        });
        let db = state.db.clone();
        let _ = tokio::task::spawn_blocking(move || db.prune_found()).await;
        if dropped > 0 {
            tracing::debug!("Dropped {dropped} relay clients");
        }
    }
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system RNG is unavailable");
    hex::encode(bytes)
}

#[cfg(test)]
mod security_tests {
    use super::*;
    #[test]
    fn registration_requires_ownership_and_fresh_signed_fields() {
        let identity = crate::identity::Identity::generate();
        let mut request = Register {
            node: identity.public_hex(),
            words: vec!["music".into()],
            issued_at: 1000,
            signature: String::new(),
            session: None,
        };
        assert!(!request.valid(1000));
        request.signature = hex::encode(identity.sign(&request.signing_bytes()));
        assert!(request.valid(1000));
        assert!(!request.valid(1061));
        request.node = crate::identity::Identity::generate().public_hex();
        assert!(!request.valid(1000));
        request.node = identity.public_hex();
        request.words.push("forged".into());
        assert!(!request.valid(1000));
        request.words.pop();
        request.session = Some("a".repeat(32));
        assert!(!request.valid(1000));
        request.session = None;
        request.words = vec![keywords::routing_group("music"); MAX_WORDS_PER_CLIENT + 1];
        request.signature = hex::encode(identity.sign(&request.signing_bytes()));
        assert!(
            !request.valid(1000),
            "routing groups cannot bypass key limits"
        );
    }

    #[tokio::test]
    async fn overflow_forwarding_is_bounded_rotates_and_cleans_cancelled_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::config::Config::isolated(dir.path().join("node")),
            &crate::auth::generate_token(),
        )
        .await
        .unwrap();
        let mut receivers = Vec::new();
        for index in 0..10 {
            let (tx, rx) = mpsc::channel(16);
            let rx = Arc::new(tokio::sync::Mutex::new(rx));
            lock(&state.relay.sessions).insert(
                format!("client-{index:02}"),
                Session {
                    node: format!("{index:064x}"),
                    words: [keywords::routing_group("obscure")].into_iter().collect(),
                    tx,
                    rx: rx.clone(),
                    last_seen: Instant::now(),
                },
            );
            receivers.push(rx);
        }
        assert!(state
            .relay
            .relayed_words()
            .contains(&keywords::routing_group("obscure")));
        let mut visited = HashSet::new();
        for _ in 0..3 {
            assert!(tokio::time::timeout(
                Duration::from_millis(20),
                state.relay.forward_search(&state, "obscure"),
            )
            .await
            .is_err());
            assert!(lock(&state.relay.answers).is_empty());
            let mut sent = 0;
            for (index, receiver) in receivers.iter().enumerate() {
                let mut receiver = receiver.lock().await;
                while let Ok(task) = receiver.try_recv() {
                    assert!(matches!(task, Task::Search { q, .. } if q == "obscure"));
                    visited.insert(index);
                    sent += 1;
                }
            }
            assert_eq!(sent, MAX_FORWARDED_CLIENTS);
        }
        assert_eq!(visited.len(), receivers.len());
    }

    #[tokio::test]
    async fn a_relay_session_cannot_answer_another_sessions_task() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::config::Config::isolated(dir.path().join("node")),
            &crate::auth::generate_token(),
        )
        .await
        .unwrap();
        let (tx, rx) = mpsc::channel(1);
        lock(&state.relay.sessions).insert(
            "attacker".into(),
            Session {
                node: "a".repeat(64),
                words: HashSet::new(),
                tx,
                rx: Arc::new(tokio::sync::Mutex::new(rx)),
                last_seen: Instant::now(),
            },
        );
        let (tx, _rx) = oneshot::channel();
        lock(&state.relay.answers).insert(1, ("victim".into(), tx));
        let body = serde_json::to_vec(&SearchResponse {
            node: "a".repeat(64),
            entries: Vec::new(),
            relayed: Vec::new(),
            bt_port: None,
        })
        .unwrap();
        let status = answer_handler(
            State(state.clone()),
            Query(AnswerQuery {
                session: "attacker".into(),
                id: 1,
            }),
            Bytes::from(body),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(lock(&state.relay.answers).contains_key(&1));
        let cleanup = PendingAnswers {
            server: &state.relay,
            ids: vec![1],
        };
        drop(cleanup);
        assert!(lock(&state.relay.answers).is_empty());
    }
}

pub async fn register_handler(
    State(state): State<AppState>,
    Json(register): Json<Register>,
) -> Result<Json<Registered>, StatusCode> {
    if !register.valid(now()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Only words exactly as this node would have normalized them
    let words: HashSet<String> = register
        .words
        .into_iter()
        .filter(|w| w.len() <= 64 && keywords::plain_words(w).collect::<Vec<_>>() == [w.as_str()])
        .take(MAX_WORDS_PER_CLIENT)
        .collect();

    let server = &state.relay;
    let session = random_id();
    {
        let mut sessions = lock(&server.sessions);
        // A signature proves key ownership; the current token additionally
        // prevents replay of a recently captured registration from replacing it.
        if let Some((token, _)) = sessions.iter().find(|(_, s)| s.node == register.node) {
            if register.session.as_deref() != Some(token.as_str()) {
                return Err(StatusCode::CONFLICT);
            }
        }
        sessions.retain(|_, s| s.node != register.node);
        if !(state.config.relay && state.reach.direct())
            || sessions.len() >= state.config.max_relay_clients
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let (tx, rx) = mpsc::channel(16);
        sessions.insert(
            session.clone(),
            Session {
                node: register.node,
                words,
                tx,
                rx: Arc::new(tokio::sync::Mutex::new(rx)),
                last_seen: Instant::now(),
            },
        );
    }
    tracing::debug!("Relaying for a new client");
    state.announce.notify_one();
    Ok(Json(Registered { session }))
}

#[derive(Deserialize)]
pub struct SessionQuery {
    session: String,
}

pub async fn poll_handler(
    State(state): State<AppState>,
    Query(query): Query<SessionQuery>,
) -> Response {
    let Some(rx) = state.relay.session_rx(&query.session) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(mut rx) = rx.try_lock() else {
        return StatusCode::CONFLICT.into_response();
    };
    let task = tokio::time::timeout(POLL_WAIT, rx.recv()).await;
    drop(rx);
    state.relay.session_rx(&query.session);
    match task {
        Ok(Some(task)) => Json(task).into_response(),
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

#[derive(Deserialize)]
pub struct AnswerQuery {
    session: String,
    id: u64,
}

pub async fn answer_handler(
    State(state): State<AppState>,
    Query(query): Query<AnswerQuery>,
    body: Bytes,
) -> StatusCode {
    if state.relay.session_rx(&query.session).is_none() {
        return StatusCode::NOT_FOUND;
    }
    let Ok(answer) = serde_json::from_slice::<SearchResponse>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if answer.entries.len() > MAX_ENTRIES_PER_ANSWER
        || answer.relayed.len() > MAX_ENTRIES_PER_ANSWER
    {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    let mut answers = lock(&state.relay.answers);
    if !answers
        .get(&query.id)
        .is_some_and(|(session, _)| session == &query.session)
    {
        return StatusCode::NOT_FOUND;
    }
    if let Some((_, tx)) = answers.remove(&query.id) {
        let _ = tx.send(answer);
    }
    StatusCode::NO_CONTENT
}

pub const MAX_ANSWER_BODY: usize = MAX_ANSWER_BYTES;

#[derive(Deserialize)]
pub struct UploadQuery {
    session: String,
    info_hash: String,
}

pub async fn lyrics_handler(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> StatusCode {
    let offered = lock(&state.relay.offered)
        .get(&query.info_hash)
        .map(|(node, _)| node.clone());
    let sender = lock(&state.relay.sessions)
        .get(&query.session)
        .map(|s| s.node.clone());
    if !is_info_hash(&query.info_hash)
        || sender.is_none()
        || sender != offered
        || !lock(&state.relay.waiting).contains_key(&query.info_hash)
    {
        return StatusCode::NOT_FOUND;
    }
    let db = state.db.clone();
    let id = tokio::task::spawn_blocking(move || db.id_for_info_hash(&query.info_hash))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(id) = id else {
        return StatusCode::NOT_FOUND;
    };
    match crate::lyrics::accept(&state, id, body.to_vec()).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

pub async fn metadata_handler(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Bytes,
) -> StatusCode {
    let offered = lock(&state.relay.offered)
        .get(&query.info_hash)
        .map(|(node, _)| node.clone());
    let sender = lock(&state.relay.sessions)
        .get(&query.session)
        .map(|s| s.node.clone());
    if !is_info_hash(&query.info_hash)
        || sender.is_none()
        || sender != offered
        || !lock(&state.relay.waiting).contains_key(&query.info_hash)
    {
        return StatusCode::NOT_FOUND;
    }
    let db = state.db.clone();
    let id = tokio::task::spawn_blocking(move || db.id_for_info_hash(&query.info_hash))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(id) = id else {
        return StatusCode::NOT_FOUND;
    };
    match crate::metadata::accept_relay(&state, id, body.to_vec()).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

/// Receives a file a client was asked for, checks it like any download, and
/// starts sharing it.
pub async fn upload_handler(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> StatusCode {
    let info_hash = query.info_hash;
    let offered_by = lock(&state.relay.offered)
        .get(&info_hash)
        .map(|(node, _)| node.clone());
    let uploader = lock(&state.relay.sessions)
        .get(&query.session)
        .map(|s| s.node.clone());
    if !is_info_hash(&info_hash)
        || uploader.is_none()
        || offered_by != uploader
        || !lock(&state.relay.waiting).contains_key(&info_hash)
    {
        return StatusCode::NOT_FOUND;
    }
    let result = receive_upload(&state, &info_hash, body).await;
    if let Err(ref err) = result {
        tracing::info!("Relayed upload of {info_hash} refused: {err:#}");
    }
    state.relay.resolve(&info_hash, result.is_ok());
    match result {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

pub(crate) async fn receive_upload(
    state: &AppState,
    info_hash: &str,
    body: Body,
) -> anyhow::Result<()> {
    let _track =
        tokio::time::timeout(Duration::from_secs(5), state.mesh.lock_track(info_hash)).await?;
    receive_upload_locked(state, info_hash, body, _track).await
}

pub(crate) async fn receive_upload_locked(
    state: &AppState,
    info_hash: &str,
    body: Body,
    track: Arc<tokio::sync::OwnedMutexGuard<()>>,
) -> anyhow::Result<()> {
    let _slot = state
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("too many simultaneous uploads"))?;
    let db = state.db.clone();
    let hash = info_hash.to_string();
    let row = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let Some(id) = db.id_for_info_hash(&hash)? else {
            return Ok(None);
        };
        db.torrent(id)
    })
    .await??
    .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
    if row.state.has_file() {
        return Ok(());
    }
    anyhow::ensure!(row.state != TrackState::Rejected, "rejected track");
    anyhow::ensure!(
        state.torrents.get(info_hash).is_none(),
        "BitTorrent download already active"
    );
    let expected_len = InfoDict::parse(&row.info, state.config.limits.max_upload_bytes)
        .map(|i| i.length)
        .ok_or_else(|| anyhow::anyhow!("bad info dictionary"))?;

    let tmp = tempfile::Builder::new()
        .prefix("relayed-")
        .tempfile_in(state.storage.tmp_dir())?;
    let (std_file, tmp_path) = tmp.into_parts();
    let mut file = tokio::fs::File::from_std(std_file);
    let mut stream = body.into_data_stream();
    let mut received = 0u64;
    while let Some(chunk) =
        tokio::time::timeout(state.config.upload_idle_timeout, stream.next()).await?
    {
        let chunk = chunk?;
        if chunk.len() as u64 > expected_len.saturating_sub(received) {
            anyhow::bail!("larger than announced");
        }
        received += chunk.len() as u64;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if received != expected_len {
        anyhow::bail!("shorter than announced");
    }

    let _permit = state.validations.clone().acquire_owned().await?;
    let state2 = state.clone();
    let row2 = row.clone();
    let ingested = tokio::task::spawn_blocking(move || -> anyhow::Result<ingest::Ingested> {
        // Keep the write reservation even if the HTTP request is cancelled
        // while this non-cancellable validation worker finishes.
        let _track = track;
        let _permit = _permit;
        let _slot = _slot;
        let (state, row) = (state2, row2);
        // The file must be exactly the torrent that was announced...
        let info = InfoDict::for_file(&tmp_path, row.format)?;
        if torrent_info::info_hash(&info.encode()) != row.info_hash {
            anyhow::bail!("the file does not match its torrent");
        }
        // ...and pass validation like any download
        let ingested = ingest::verify_download(
            &tmp_path,
            row.format,
            &row.sha256,
            &row.audio_sha256,
            row.duration_ms,
            &state.storage.tmp_dir(),
            &state.config.limits,
        )
        .map_err(|r| anyhow::anyhow!("{r}"))?;
        state
            .storage
            .store_track(tmp_path, &row.info_hash, row.format)?;
        state
            .db
            .set_peaks(row.id, &serde_json::to_string(&ingested.peaks)?)?;
        state
            .db
            .set_verified_duration(row.id, ingested.duration_ms)?;
        state.db.set_state(row.id, TrackState::Held)?;
        state.tracks_changed();
        Ok(ingested)
    })
    .await?;
    if let Err(err) = ingested {
        let db = state.db.clone();
        let id = row.id;
        let _ = tokio::task::spawn_blocking(move || db.set_state(id, TrackState::Rejected)).await;
        return Err(err);
    }
    state
        .torrents
        .add(&row.info_hash, &row.info, Vec::new())
        .await?;
    tracing::info!(
        "Fetched {} for another node through relaying",
        row.info_hash
    );
    crate::library::make_room(state).await;
    Ok(())
}

#[derive(Deserialize)]
pub struct FetchQuery {
    info_hash: String,
    #[serde(default = "default_fetch_hops")]
    hops: u8,
}
fn default_fetch_hops() -> u8 {
    3
}

/// Has a relayed track uploaded here, waiting until it is validated and
/// shared, so the asking node can download it over BitTorrent.
pub async fn fetch_handler(
    State(state): State<AppState>,
    Query(query): Query<FetchQuery>,
) -> StatusCode {
    let info_hash = query.info_hash;
    if !is_info_hash(&info_hash) {
        return StatusCode::BAD_REQUEST;
    }
    let db = state.db.clone();
    let hash = info_hash.clone();
    let known = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        Ok(match db.id_for_info_hash(&hash)? {
            Some(id) => db.torrent(id)?.map(|r| r.state),
            None => None,
        })
    })
    .await
    .ok()
    .and_then(Result::ok)
    .flatten();
    match known {
        Some(s) if s.has_file() => return StatusCode::NO_CONTENT,
        Some(TrackState::Rejected) | None => return StatusCode::NOT_FOUND,
        _ => {}
    }

    let Some(node) = lock(&state.relay.offered)
        .get(&info_hash)
        .map(|(node, _)| node.clone())
    else {
        return match crate::mesh::fetch(&state, &info_hash, query.hops).await {
            Ok(()) => StatusCode::NO_CONTENT,
            Err(_) => StatusCode::NOT_FOUND,
        };
    };
    let (tx, rx) = oneshot::channel();
    let first = {
        let mut waiting = lock(&state.relay.waiting);
        let waiters = waiting.entry(info_hash.clone()).or_default();
        waiters.push(tx);
        waiters.len() == 1
    };
    if first {
        let sent = lock(&state.relay.sessions)
            .values()
            .find(|s| s.node == node)
            .map(|s| {
                s.tx.try_send(Task::Upload {
                    info_hash: info_hash.clone(),
                })
                .is_ok()
            })
            .unwrap_or(false);
        if !sent {
            state.relay.resolve(&info_hash, false);
        }
    }
    match tokio::time::timeout(FETCH_TIMEOUT, rx).await {
        Ok(Ok(true)) => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::BAD_GATEWAY,
        Err(_) => {
            lock(&state.relay.waiting).remove(&info_hash);
            StatusCode::GATEWAY_TIMEOUT
        }
    }
}

/// Asks the relay at `relay` to fetch a track from its client.
pub async fn request_fetch(
    state: &AppState,
    relay: SocketAddr,
    info_hash: &str,
) -> anyhow::Result<()> {
    let response = state
        .http
        .post(format!("http://{relay}{FETCH_PATH}?info_hash={info_hash}"))
        .timeout(FETCH_TIMEOUT + Duration::from_secs(10))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("the relay could not fetch the track ({status})");
    }
    Ok(())
}

fn candidates_for_round(nodes: &[SocketAddr], round: usize) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    let mut candidates: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|addr| seen.insert(*addr))
        .take(MAX_RELAY_CANDIDATES)
        .collect();
    if !candidates.is_empty() {
        let start = round.wrapping_mul(MAX_REGISTRATION_ATTEMPTS) % candidates.len();
        candidates.rotate_left(start);
    }
    candidates
}

/// The client side: finds a relay among `nodes` (configured relays first on
/// the initial round, then rotating so failed registrations cannot monopolize it)
/// and keeps up to two independent reverse connections. Both share the same
/// task budget. Each worker retries its disjoint candidate half with backoff.
pub async fn run_client(state: &AppState, nodes: &[SocketAddr], duration: Duration, round: usize) {
    let deadline = Instant::now() + duration;
    let clients = async {
        tokio::join!(
            client_worker(state, nodes, deadline, round, 0),
            client_worker(state, nodes, deadline, round, 1)
        );
    };
    let _ = tokio::time::timeout(duration, clients).await;
}

async fn client_worker(
    state: &AppState,
    nodes: &[SocketAddr],
    deadline: Instant,
    mut round: usize,
    worker: usize,
) {
    let me = state.identity.public_hex();
    let mut nodes = nodes.to_vec();
    while Instant::now() < deadline {
        let candidates = candidates_for_round(&nodes, round);
        for candidate in candidates
            .into_iter()
            .skip(worker)
            .step_by(2)
            .take(MAX_REGISTRATION_ATTEMPTS / 2)
        {
            if state.relay.outgoing_peers().contains(&candidate) {
                continue;
            }
            let attempt = async {
                let hello = fetch_hello(state, candidate).await?;
                anyhow::ensure!(hello.relay && hello.node != me, "relay unavailable");
                serve_through(state, candidate, deadline).await
            };
            match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), attempt)
                .await
            {
                Ok(Ok(())) | Err(_) => return,
                Ok(Err(err)) => tracing::info!("Relay {candidate} unavailable: {err:#}"),
            }
        }
        round = round.wrapping_add(1);
        let wait = RETRY_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
        tokio::time::sleep(wait).await;
        if Instant::now() < deadline {
            nodes = crate::reach::other_nodes(state).await;
        }
    }
}

async fn own_words(state: &AppState) -> Vec<String> {
    let db = state.db.clone();
    let texts = tokio::task::spawn_blocking(move || db.held_texts())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    keywords::announced_keys(texts.iter().map(|t| {
        [
            t[0].as_deref(),
            t[1].as_deref(),
            t[2].as_deref(),
            t[3].as_deref(),
        ]
    }))
}

async fn register(
    state: &AppState,
    relay: SocketAddr,
    previous: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = Register {
        node: state.identity.public_hex(),
        words: own_words(state).await,
        issued_at: now(),
        signature: String::new(),
        session: previous.map(str::to_owned),
    };
    request.signature = hex::encode(state.identity.sign(&request.signing_bytes()));
    let response = state
        .http
        .post(format!("http://{relay}{REGISTER_PATH}"))
        .json(&request)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let registered: Registered = read_json(response, 4096).await?;
    Ok(registered.session)
}

async fn serve_through(
    state: &AppState,
    relay: SocketAddr,
    deadline: Instant,
) -> anyhow::Result<()> {
    let mut changes = state.changes.subscribe();
    changes.mark_unchanged();
    // Reserve before the handshake so concurrent recovery attempts cannot
    // replace each other's registration on the same relay.
    {
        let mut peers = lock(&state.relay.outgoing.peers);
        anyhow::ensure!(
            peers.len() < 2 && !peers.contains_key(&relay),
            "relay already in use"
        );
        peers.insert(relay, false);
    }
    let _connection = Connection { state, peer: relay };
    let mut session = register(state, relay, None).await?;
    let mut registered_at = Instant::now();
    tracing::info!("Others reach this node through the relay {relay}");
    {
        let mut peers = lock(&state.relay.outgoing.peers);
        peers.insert(relay, true);
        state.reach.set(Reachability::Relay, Some(relay));
    }
    state.announce.notify_one();

    let mut tasks = tokio::task::JoinSet::new();
    let mut next_poll = tokio::time::Instant::now();
    while Instant::now() < deadline {
        while tasks.try_join_next().is_some() {}
        tokio::time::sleep_until(next_poll).await;
        next_poll = tokio::time::Instant::now() + MIN_POLL_INTERVAL;
        if registered_at.elapsed() > REREGISTER_INTERVAL {
            session = register(state, relay, Some(&session)).await?;
            registered_at = Instant::now();
        }
        let poll = state
            .http
            .get(format!("http://{relay}{POLL_PATH}?session={session}"))
            .timeout(POLL_WAIT + Duration::from_secs(10))
            .send();
        let response = tokio::select! {
            response = poll => response?,
            // New or removed tracks: the relay needs the new words now, not
            // at the next periodic registration
            _ = changes.changed() => {
                changes.mark_unchanged();
                session = register(state, relay, Some(&session)).await?;
                registered_at = Instant::now();
                continue;
            }
        };
        match response.status() {
            StatusCode::NO_CONTENT => continue,
            StatusCode::NOT_FOUND => {
                // The relay forgot us, for example after a restart
                session = register(state, relay, None).await?;
                registered_at = Instant::now();
                continue;
            }
            s if !s.is_success() => anyhow::bail!("poll failed ({s})"),
            _ => {}
        }
        let task: Task = read_json(response, 64 * 1024).await?;
        if tasks.len() >= MAX_CLIENT_TASKS {
            // Keep polling to preserve the relay lease during long uploads,
            // but do not queue unbounded work. The relay's existing task
            // timeout lets a requester retry instead of exhausting this node.
            tracing::debug!("Relay client busy; declining additional work");
            continue;
        }
        let Ok(permit) = state.relay.outgoing.tasks.clone().try_acquire_owned() else {
            continue;
        };
        let state = state.clone();
        let session = session.clone();
        tasks.spawn(async move {
            let _permit = permit;
            if let Err(err) = handle_task(&state, relay, &session, task).await {
                tracing::debug!("Relay task failed: {err:#}");
            }
        });
    }
    Ok(())
}

async fn handle_task(
    state: &AppState,
    relay: SocketAddr,
    session: &str,
    task: Task,
) -> anyhow::Result<()> {
    match task {
        Task::Search { id, q } => {
            let response = crate::p2p::local_answer(state, &q, true).await;
            state
                .http
                .post(format!(
                    "http://{relay}{ANSWER_PATH}?session={session}&id={id}"
                ))
                .json(&response)
                .timeout(Duration::from_secs(10))
                .send()
                .await?
                .error_for_status()?;
        }
        Task::Upload { info_hash } => {
            if !is_info_hash(&info_hash) {
                anyhow::bail!("bad info hash");
            }
            let db = state.db.clone();
            let hash = info_hash.clone();
            let row = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
                match db.id_for_info_hash(&hash)? {
                    Some(id) => db.torrent(id),
                    None => Ok(None),
                }
            })
            .await??
            .filter(|r| r.state.has_file())
            .ok_or_else(|| anyhow::anyhow!("asked for a track this node doesn't have"))?;
            let data =
                tokio::fs::read(state.storage.media_path(&row.info_hash, row.format)).await?;
            if let Ok(variants) = crate::metadata::list(state, row.id, false).await {
                for variant in variants.into_iter().filter(|v| v.selected) {
                    if let Ok(Some(data)) = crate::metadata::data(state, row.id, variant.hash).await
                    {
                        use base64::Engine;
                        let body = serde_json::json!({"kind":variant.kind,"data":base64::engine::general_purpose::STANDARD.encode(data)});
                        let _=state.http.post(format!("http://{relay}{METADATA_PATH}?session={session}&info_hash={info_hash}"))
                            .json(&body).timeout(Duration::from_secs(5)).send().await;
                    }
                }
            }
            if let Some(lyrics) = crate::lyrics::document(state, &info_hash).await {
                // Send before audio resolves the outstanding relay request.
                // Old relays may not support attachments; audio still works.
                let _ = state
                    .http
                    .post(format!(
                        "http://{relay}{LYRICS_PATH}?session={session}&info_hash={info_hash}"
                    ))
                    .body(lyrics)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
            }
            state
                .http
                .post(format!(
                    "http://{relay}{UPLOAD_PATH}?session={session}&info_hash={info_hash}"
                ))
                .body(data)
                .timeout(Duration::from_secs(10 * 60))
                .send()
                .await?
                .error_for_status()?;
            tracing::debug!("Uploaded {info_hash} to the relay");
        }
    }
    Ok(())
}

#[cfg(test)]
mod client_limits_tests {
    use super::*;

    #[test]
    fn failed_registration_candidates_rotate_without_duplicates() {
        let nodes: Vec<SocketAddr> = (1..=7)
            .map(|n| SocketAddr::from(([127, 0, 0, 1], 10000 + n)))
            .collect();
        let duplicate_nodes: Vec<_> = nodes.iter().chain(&nodes).copied().collect();
        assert_eq!(candidates_for_round(&duplicate_nodes, 0), nodes);
        assert_eq!(candidates_for_round(&duplicate_nodes, 1)[0], nodes[4]);
        assert_eq!(candidates_for_round(&duplicate_nodes, 2)[0], nodes[1]);
        assert!(candidates_for_round(&[], usize::MAX).is_empty());
    }

    async fn exercise_fast_relay(send_tasks: bool, duration: Duration) -> (usize, usize) {
        use axum::{
            routing::{get, post},
            Router,
        };
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let answers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let submitted = answers.clone();
        let polls = count.clone();
        let router = Router::new()
            .route(
                REGISTER_PATH,
                post(|| async {
                    Json(Registered {
                        session: "fixture".into(),
                    })
                }),
            )
            .route(
                POLL_PATH,
                get(move || {
                    let polls = polls.clone();
                    async move {
                        let id = polls.fetch_add(1, Ordering::Relaxed) as u64;
                        if send_tasks {
                            Json(Task::Search {
                                id,
                                q: "song".into(),
                            })
                            .into_response()
                        } else {
                            StatusCode::NO_CONTENT.into_response()
                        }
                    }
                }),
            )
            .route(
                ANSWER_PATH,
                post(move || {
                    submitted.fetch_add(1, Ordering::Relaxed);
                    async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        StatusCode::NO_CONTENT
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::config::Config::isolated(dir.path().join("node")),
            &crate::auth::generate_token(),
        )
        .await
        .unwrap();
        tokio::time::timeout(
            duration + Duration::from_secs(3),
            serve_through(&state, addr, Instant::now() + duration),
        )
        .await
        .unwrap()
        .unwrap();
        server.abort();
        (
            count.load(Ordering::Relaxed),
            answers.load(Ordering::Relaxed),
        )
    }

    #[tokio::test]
    async fn immediate_empty_polls_cannot_create_a_busy_loop() {
        let (polls, _) = exercise_fast_relay(false, Duration::from_millis(1200)).await;
        assert!((2..=4).contains(&polls), "unexpected poll count: {polls}");
    }

    #[tokio::test]
    async fn a_fast_relay_cannot_spawn_unbounded_blocked_tasks() {
        let (polls, answers) = exercise_fast_relay(true, Duration::from_millis(1200)).await;
        assert!(
            (3..=4).contains(&polls),
            "polling must maintain the session while busy"
        );
        assert_eq!(answers, MAX_CLIENT_TASKS);
    }
}
