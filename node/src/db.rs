// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite storage for the tracks this node knows about.
//!
//! A node only knows about tracks it has itself (shared or downloaded) and
//! the results of recent searches. There is no copy of the network's
//! catalog anywhere.
//!
//! All queries use bound parameters; text from other nodes never becomes
//! SQL. Search input is additionally reduced to quoted FTS5 terms so it
//! cannot use the FTS query syntax either.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use hopea_common::{
    api::{SearchOrder, Track},
    AudioFormat,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{
    entry::Entry,
    fuzzy::{Fields, Query},
    identity::fingerprint,
};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    info_hash TEXT NOT NULL UNIQUE,
    publisher TEXT NOT NULL,
    publisher_name TEXT,
    body BLOB NOT NULL,
    sig BLOB NOT NULL,
    info BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    track_number INTEGER,
    year INTEGER,
    genre TEXT,
    duration_ms INTEGER NOT NULL,
    format TEXT NOT NULL,
    codec TEXT NOT NULL,
    sample_rate INTEGER NOT NULL,
    channels INTEGER NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    audio_sha256 TEXT NOT NULL,
    state INTEGER NOT NULL,
    has_cover INTEGER NOT NULL DEFAULT 0,
    cover_sha256 TEXT,
    peaks TEXT,
    seen_at INTEGER NOT NULL,
    used_at INTEGER NOT NULL DEFAULT 0,
    plays INTEGER NOT NULL DEFAULT 0,
    favorite_at INTEGER
);

CREATE TABLE IF NOT EXISTS metadata_variants (
    audio TEXT NOT NULL, hash TEXT NOT NULL, kind TEXT NOT NULL, data BLOB NOT NULL,
    selected INTEGER NOT NULL DEFAULT 0, contributed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(audio, hash)
);
CREATE TABLE IF NOT EXISTS metadata_checks (audio TEXT PRIMARY KEY, checked_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS metadata_sources (
    audio TEXT NOT NULL, hash TEXT NOT NULL, peer TEXT NOT NULL, seen_at INTEGER NOT NULL,
    PRIMARY KEY(audio, hash, peer)
);
CREATE INDEX IF NOT EXISTS entries_state ON entries(state);
CREATE INDEX IF NOT EXISTS entries_audio ON entries(audio_sha256);

CREATE TABLE IF NOT EXISTS sources (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    addr TEXT NOT NULL,
    seen_at INTEGER NOT NULL,
    relay TEXT,
    PRIMARY KEY (entry_id, addr)
);

CREATE TABLE IF NOT EXISTS known_peers (
    addr TEXT PRIMARY KEY,
    seen_at INTEGER NOT NULL,
    retry_at INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0
);

CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    title, artist, album, album_artist, genre,
    content = 'entries', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, title, artist, album, album_artist, genre)
    VALUES (new.id, new.title, new.artist, new.album, new.album_artist, new.genre);
END;

CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, title, artist, album, album_artist, genre)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.album_artist, old.genre);
END;
"#;

const TRACK_COLUMNS: &str = "e.id, e.title, e.artist, e.album, e.album_artist, e.track_number, \
     e.year, e.genre, e.duration_ms, e.format, e.codec, e.sample_rate, e.channels, e.size, \
     e.sha256, (e.has_cover OR e.cover_sha256 IS NOT NULL OR EXISTS(SELECT 1 FROM metadata_variants v WHERE v.audio=e.audio_sha256 AND v.kind='cover' AND v.selected=1)), e.publisher, e.publisher_name, e.created_at, e.state, e.info_hash, \
     (e.favorite_at IS NOT NULL), \
     (SELECT COUNT(DISTINCT COALESCE(s.node, s.relay, s.addr)) FROM sources s JOIN entries copy ON copy.id=s.entry_id WHERE copy.audio_sha256=e.audio_sha256 AND s.seen_at>=CAST(strftime('%s','now') AS INTEGER)-60) + (e.state IN (2, 3))";

const DISCOVER_FILTER: &str = "e.state NOT IN (2, 3, 4) AND e.favorite_at IS NULL AND NOT EXISTS (SELECT 1 FROM entries held WHERE held.state IN (2, 3) AND (held.sha256 = e.sha256 OR (held.audio_sha256 IS NOT NULL AND held.audio_sha256 = e.audio_sha256))) AND EXISTS (SELECT 1 FROM sources available WHERE available.entry_id = e.id AND available.seen_at >= CAST(strftime('%s', 'now') AS INTEGER) - 60)";

/// Longest accepted search query, in characters
const MAX_QUERY_CHARS: usize = 200;
const MAX_QUERY_TERMS: usize = 16;
/// Search results nobody played are forgotten after this long
const FOUND_TTL_SECS: i64 = 7 * 24 * 60 * 60;
/// Sources remembered per track
const MAX_SOURCES: i64 = 32;
/// Search results remembered at most
pub const MAX_FOUND: i64 = 20_000;

/// `(body, signature, info dictionary)` of a stored entry; the info
/// dictionary is empty until known
pub type StoredEntry = (Vec<u8>, Vec<u8>, Vec<u8>);

/// What this node has of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Known from a search; nothing downloaded
    Found = 0,
    /// Being downloaded, not yet validated
    Fetching = 1,
    /// Downloaded, validated, and shared on
    Held = 2,
    /// Shared by this node
    Own = 3,
    /// Downloaded but failed validation; never fetched again
    Rejected = 4,
}

impl State {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => State::Fetching,
            2 => State::Held,
            3 => State::Own,
            4 => State::Rejected,
            _ => State::Found,
        }
    }

    /// Whether this node has the whole, validated file.
    pub fn has_file(self) -> bool {
        matches!(self, State::Held | State::Own)
    }
}

#[derive(Debug, Clone)]
pub struct TrackRow {
    pub track: Track,
    pub state: State,
    pub info_hash: String,
}

/// What is needed to add a known track to the BitTorrent session.
#[derive(Debug, Clone)]
pub struct TorrentRow {
    pub id: i64,
    pub info_hash: String,
    pub info: Vec<u8>,
    pub format: AudioFormat,
    pub state: State,
    pub sha256: String,
    pub audio_sha256: String,
    pub duration_ms: u64,
}

/// A cheaply cloneable handle to the database.
///
/// Every method is blocking; async code calls them through
/// `tokio::task::spawn_blocking`.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    /// This node's public key, to tell its own tracks apart
    me: Arc<str>,
    scores: Arc<Mutex<ScoreCache>>,
}

// Only fuzzy scores are cached. Live availability and peer popularity are
// always read again, so offline tracks never survive through this cache.
const SCORE_CACHE_BYTES: usize = 2 * 1024 * 1024;
#[derive(Default)]
struct ScoreCache {
    entries: VecDeque<((u64, i64), String, Arc<Vec<(f64, i64)>>)>,
    bytes: usize,
}
impl ScoreCache {
    fn get(&mut self, revision: (u64, i64), key: &str) -> Option<Arc<Vec<(f64, i64)>>> {
        let position = self
            .entries
            .iter()
            .position(|(r, k, _)| *r == revision && k == key)?;
        let entry = self.entries.remove(position)?;
        let result = entry.2.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    fn insert(&mut self, revision: (u64, i64), key: String, scores: Arc<Vec<(f64, i64)>>) {
        let size = key.capacity().saturating_add(
            scores
                .capacity()
                .saturating_mul(std::mem::size_of::<(f64, i64)>()),
        );
        if size > SCORE_CACHE_BYTES {
            return;
        }
        while self.entries.len() >= 4 || self.bytes.saturating_add(size) > SCORE_CACHE_BYTES {
            let Some((_, key, old)) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(
                key.capacity() + old.capacity() * std::mem::size_of::<(f64, i64)>(),
            );
        }
        self.bytes += size;
        self.entries.push_back((revision, key, scores));
    }
}
fn search_revision(conn: &Connection) -> rusqlite::Result<(u64, i64)> {
    Ok((
        conn.total_changes(),
        conn.pragma_query_value(None, "data_version", |r| r.get(0))?,
    ))
}

/// What is known about a track's cover art.
#[derive(Debug, Clone)]
pub struct CoverState {
    pub info_hash: String,
    /// What the entry says its hash is
    pub sha256: Option<String>,
    /// The file is on this node
    pub here: bool,
    pub own: bool,
}

/// A node that has a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// Its BitTorrent address
    pub addr: SocketAddr,
    /// For a track relayed from a node nobody can reach: the search address
    /// of the relay, which has to fetch the track before it can send it
    pub relay: Option<SocketAddr>,
    /// The search address of the node that answered, which can send the
    /// track's info dictionary
    pub node: Option<SocketAddr>,
}

/// Brings databases created by earlier versions up to date.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let has_cover_hash: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('entries') WHERE name = 'cover_sha256'")?
        .exists([])?;
    let has_duration = conn
        .prepare("SELECT 1 FROM pragma_table_info('entries') WHERE name='verified_duration_ms'")?
        .exists([])?;
    if !has_duration {
        conn.execute_batch("ALTER TABLE entries ADD COLUMN verified_duration_ms INTEGER")?;
    }
    if !has_cover_hash {
        conn.execute_batch("ALTER TABLE entries ADD COLUMN cover_sha256 TEXT")?;
    }
    for column in ["relay", "node"] {
        let exists: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('sources') WHERE name = ?1")?
            .exists([column])?;
        if !exists {
            conn.execute_batch(&format!("ALTER TABLE sources ADD COLUMN {column} TEXT"))?;
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS preferences_migrations (name TEXT PRIMARY KEY);
        INSERT OR IGNORE INTO preferences_migrations VALUES ('explicit-favorites');",
    )?;
    if conn.changes() > 0 {
        conn.execute(
            "UPDATE entries SET favorite_at = seen_at WHERE state = 3 AND favorite_at IS NULL",
            [],
        )?;
    }
    Ok(())
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Db {
    pub fn open(path: &std::path::Path, me: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let has_contributed = conn
            .prepare("PRAGMA table_info(metadata_variants)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .any(|r| r.ok().as_deref() == Some("contributed"));
        if !has_contributed {
            conn.execute(
                "ALTER TABLE metadata_variants ADD COLUMN contributed INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            me: me.into(),
            scores: Arc::default(),
        })
    }

    pub(crate) fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records a verified entry. If the track is already known, its state is
    /// raised to `state` if that is further along and the entry is kept as it
    /// was. Returns the track's id.
    pub fn insert_entry(&self, entry: &Entry, state: State) -> rusqlite::Result<i64> {
        let t = &entry.track;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO entries (info_hash, publisher, publisher_name, body, sig, info,
                 created_at, title, artist, album, album_artist, track_number, year, genre,
                 duration_ms, format, codec, sample_rate, channels, size, sha256, audio_sha256,
                 state, seen_at, cover_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
             ON CONFLICT(info_hash) DO UPDATE SET seen_at = excluded.seen_at,
                 info = CASE WHEN length(info) = 0 THEN excluded.info ELSE info END,
                 -- A newer entry from the same publisher replaces the old
                 -- one for a cover update. Other publishers and stale
                 -- versions cannot replace the signed artwork.
                 -- (SQLite evaluates every expression here against the
                 -- row as it was)
                 body = CASE WHEN excluded.publisher = publisher
                     AND excluded.created_at > created_at THEN excluded.body ELSE body END,
                 sig = CASE WHEN excluded.publisher = publisher
                     AND excluded.created_at > created_at THEN excluded.sig ELSE sig END,
                 has_cover = CASE WHEN excluded.publisher = publisher
                     AND excluded.created_at > created_at AND state != 3
                     AND excluded.cover_sha256 IS NOT cover_sha256 THEN 0 ELSE has_cover END,
                 cover_sha256 = CASE WHEN excluded.publisher = publisher
                     AND excluded.created_at > created_at
                     THEN excluded.cover_sha256 ELSE cover_sha256 END,
                 created_at = CASE WHEN excluded.publisher = publisher
                     AND excluded.created_at > created_at
                     THEN excluded.created_at ELSE created_at END",
            params![
                entry.info_hash,
                entry.publisher,
                entry.name,
                entry.body,
                entry.sig,
                entry.info_bytes.as_deref().unwrap_or_default(),
                entry.created_at,
                t.title,
                t.artist,
                t.album,
                t.album_artist,
                t.track_number,
                t.year,
                t.genre,
                t.duration_ms as i64,
                format_name(t.format),
                t.codec,
                t.sample_rate,
                t.channels,
                t.size as i64,
                t.sha256,
                t.audio_sha256,
                state as i64,
                now(),
                entry.cover_sha256,
            ],
        )?;
        let (id, current): (i64, i64) = conn.query_row(
            "SELECT id, state FROM entries WHERE info_hash = ?1",
            params![entry.info_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let current = State::from_i64(current);
        if current != state && current != State::Rejected && (state as i64) > (current as i64) {
            conn.execute(
                "UPDATE entries SET state = ?2 WHERE id = ?1",
                params![id, state as i64],
            )?;
        }
        Ok(id)
    }

    pub fn set_state(&self, id: i64, state: State) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET state = ?2 WHERE id = ?1",
            params![id, state as i64],
        )?;
        Ok(())
    }

    pub fn set_peaks(&self, id: i64, peaks_json: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET peaks = ?2 WHERE id = ?1",
            params![id, peaks_json],
        )?;
        Ok(())
    }

    /// What is known about a track's cover art.
    pub fn cover_state(&self, id: i64) -> rusqlite::Result<Option<CoverState>> {
        self.conn()
            .query_row(
                "SELECT info_hash, cover_sha256, has_cover, state FROM entries WHERE id = ?1",
                params![id],
                |r| {
                    Ok(CoverState {
                        info_hash: r.get(0)?,
                        sha256: r.get(1)?,
                        here: r.get(2)?,
                        own: r.get::<_, i64>(3)? == State::Own as i64,
                    })
                },
            )
            .optional()
    }

    /// Whether this node has the cover art of a track, to send to others.
    pub fn has_cover_for(&self, info_hash: &str) -> rusqlite::Result<bool> {
        self.conn()
            .prepare("SELECT 1 FROM entries WHERE info_hash = ?1 AND has_cover = 1")?
            .exists(params![info_hash])
    }

    /// Replaces the entry of an own track with a newer one, such as one
    /// naming new cover art.
    pub fn replace_own_entry(&self, id: i64, entry: &Entry) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET body = ?2, sig = ?3, created_at = ?4, cover_sha256 = ?5
             WHERE id = ?1 AND state = 3",
            params![
                id,
                entry.body,
                entry.sig,
                entry.created_at,
                entry.cover_sha256
            ],
        )?;
        Ok(())
    }

    /// Own tracks with cover art whose entry doesn't name it yet, shared
    /// before covers were passed on.
    pub fn own_covers_unannounced(&self) -> rusqlite::Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM entries WHERE state = 3 AND has_cover = 1 AND cover_sha256 IS NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    pub fn clear_cached_cover(&self, id: i64) -> rusqlite::Result<()> {
        self.conn()
            .execute("UPDATE entries SET has_cover = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Retain only endpoints we contacted ourselves, never addresses advertised
    /// by a remote peer. Private addresses must not survive a network change.
    pub(crate) fn remember_peer(&self, addr: SocketAddr) -> rusqlite::Result<()> {
        if !crate::p2p::public_discovery_address(addr) {
            return Ok(());
        }
        let conn = self.conn();
        conn.execute(
            "INSERT INTO known_peers(addr, seen_at) VALUES (?1, ?2)
             ON CONFLICT(addr) DO UPDATE SET seen_at=excluded.seen_at, retry_at=0, failures=0",
            params![addr.to_string(), now()],
        )?;
        conn.execute(
            "DELETE FROM known_peers WHERE seen_at < ?1 OR addr NOT IN
             (SELECT addr FROM known_peers ORDER BY seen_at DESC, addr LIMIT 128)",
            [now() - 7 * 24 * 60 * 60],
        )?;
        Ok(())
    }

    pub(crate) fn peer_failed(&self, addr: SocketAddr) -> rusqlite::Result<()> {
        // Failed attempts never extend the seven-day lifetime. Back off from
        // two minutes to at most thirty minutes, without creating new rows.
        self.conn().execute(
            "UPDATE known_peers SET retry_at=?2 + MIN(1800, 120 * (1 << MIN(failures, 4))),
             failures=MIN(failures+1, 5) WHERE addr=?1",
            params![addr.to_string(), now()],
        )?;
        Ok(())
    }

    pub(crate) fn known_peers(&self, ipv4_only: bool) -> rusqlite::Result<Vec<SocketAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT addr FROM known_peers WHERE seen_at >= ?1 AND seen_at <= ?2 AND retry_at <= ?2
             ORDER BY seen_at DESC, addr LIMIT 128",
        )?;
        let rows = stmt.query_map(params![now() - 7 * 24 * 60 * 60, now()], |r| {
            r.get::<_, String>(0)
        })?;
        let mut peers = Vec::new();
        for row in rows {
            if let Ok(addr) = row?.parse::<SocketAddr>() {
                if crate::p2p::public_discovery_address(addr) && (!ipv4_only || addr.is_ipv4()) {
                    peers.push(addr);
                }
            }
        }
        Ok(peers)
    }

    pub fn known_search_nodes(&self, query: &str) -> rusqlite::Result<Vec<SocketAddr>> {
        let Some(query) = Query::new(query) else {
            return Ok(Vec::new());
        };
        let ids = self.fuzzy_ids(&query, "0, 1", SearchOrder::BestMatch)?;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(relay, node) FROM sources WHERE entry_id = ?1 ORDER BY seen_at DESC",
        )?;
        let mut nodes = Vec::new();
        for id in ids.into_iter().take(5000) {
            for node in stmt.query_map([id], |r| r.get::<_, Option<String>>(0))? {
                if let Some(node) = node?.and_then(|n| n.parse().ok()) {
                    if !nodes.contains(&node) {
                        nodes.push(node);
                    }
                }
                if nodes.len() >= 40 {
                    return Ok(nodes);
                }
            }
        }
        Ok(nodes)
    }

    pub fn forget_source_node(&self, node: SocketAddr) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM sources WHERE node = ?1 OR relay = ?1",
            [node.to_string()],
        )?;
        Ok(())
    }

    pub fn forget_track_source(&self, id: i64, node: SocketAddr) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM sources WHERE entry_id = ?1 AND (node = ?2 OR relay = ?2)",
            params![id, node.to_string()],
        )?;
        Ok(())
    }

    pub fn forget_query_sources(&self, node: SocketAddr, query: &str) -> rusqlite::Result<()> {
        let Some(query) = Query::new(query) else {
            return Ok(());
        };
        let ids = self.fuzzy_ids(&query, "0, 1", SearchOrder::BestMatch)?;
        let conn = self.conn();
        let mut stmt =
            conn.prepare("DELETE FROM sources WHERE entry_id = ?1 AND (node = ?2 OR relay = ?2)")?;
        for id in ids {
            stmt.execute(params![id, node.to_string()])?;
        }
        Ok(())
    }

    pub fn available_discovery_ids(&self, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT 1 FROM entries e WHERE e.id = ?1 AND {DISCOVER_FILTER}"
        ))?;
        let mut available = Vec::new();
        for &id in ids.iter().take(200) {
            if stmt.exists([id])? {
                available.push(id);
            }
        }
        Ok(available)
    }

    pub fn set_has_cover(&self, id: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET has_cover = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn available_playback_ids(&self, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM entries e WHERE e.id = ?1 AND
            (e.state IN (2, 3) OR (e.state IN (0, 1) AND EXISTS
              (SELECT 1 FROM sources s WHERE s.entry_id = e.id AND s.seen_at >= ?2)))",
        )?;
        ids.iter()
            .take(200)
            .filter_map(|&id| match stmt.exists(params![id, now() - 60]) {
                Ok(true) => Some(Ok(id)),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    pub fn mark_used(&self, id: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET used_at = ?2, seen_at = ?2, plays = plays + 1 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    /// Remembers that a node has the track, keeping only the most recent
    /// sources. See [`Source`].
    pub fn add_source(&self, id: i64, source: Source) -> rusqlite::Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO sources (entry_id, addr, seen_at, relay, node) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO UPDATE SET seen_at = excluded.seen_at, relay = excluded.relay,
                 node = excluded.node",
            params![
                id,
                source.addr.to_string(),
                now(),
                source.relay.map(|r| r.to_string()),
                source.node.map(|r| r.to_string())
            ],
        )?;
        conn.execute(
            "DELETE FROM sources WHERE entry_id = ?1 AND addr NOT IN
                 (SELECT addr FROM sources WHERE entry_id = ?1 ORDER BY seen_at DESC LIMIT ?2)",
            params![id, MAX_SOURCES],
        )?;
        Ok(())
    }

    /// Nodes known to have the track, most recent first.
    pub fn sources(&self, id: i64) -> rusqlite::Result<Vec<Source>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT addr, relay, node FROM sources WHERE entry_id = ?1 ORDER BY seen_at DESC",
        )?;
        let sources = stmt
            .query_map(params![id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .filter_map(|row| {
                let (addr, relay, node) = row.ok()?;
                Some(Source {
                    addr: addr.parse().ok()?,
                    relay: match relay {
                        Some(r) => Some(r.parse().ok()?),
                        None => None,
                    },
                    node: node.and_then(|n| n.parse().ok()),
                })
            })
            .collect();
        Ok(sources)
    }

    /// Stores a track's info dictionary, once fetched and checked.
    pub fn set_info(&self, id: i64, info: &[u8]) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE entries SET info = ?2 WHERE id = ?1",
            params![id, info],
        )?;
        Ok(())
    }

    /// The info dictionary of a track, if this node knows it.
    pub fn info_for(&self, info_hash: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        self.conn()
            .query_row(
                "SELECT info FROM entries WHERE info_hash = ?1 AND length(info) > 0",
                params![info_hash],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn track(&self, id: i64) -> rusqlite::Result<Option<TrackRow>> {
        let sql = format!("SELECT {TRACK_COLUMNS} FROM entries e WHERE e.id = ?1");
        let me = self.me.clone();
        self.conn()
            .query_row(&sql, params![id], |r| track_row(r, &me))
            .optional()
    }

    pub fn torrent(&self, id: i64) -> rusqlite::Result<Option<TorrentRow>> {
        self.conn()
            .query_row(
                "SELECT id, info_hash, info, format, state, sha256, audio_sha256, duration_ms
                 FROM entries WHERE id = ?1",
                params![id],
                torrent_row,
            )
            .optional()
    }

    /// Tracks whose files are, or are becoming, available on this node.
    pub fn torrents_to_resume(&self) -> rusqlite::Result<Vec<TorrentRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, info_hash, info, format, state, sha256, audio_sha256, duration_ms
             FROM entries WHERE state IN (1, 2, 3)",
        )?;
        let rows = stmt.query_map([], torrent_row)?;
        rows.collect()
    }

    /// The stored entry of a track, as it was received or created.
    pub fn entry_bytes(&self, id: i64) -> rusqlite::Result<Option<(Vec<u8>, Vec<u8>)>> {
        self.conn()
            .query_row(
                "SELECT body, sig FROM entries WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    pub fn own_track_with_audio(&self, audio_sha256: &str) -> rusqlite::Result<Option<i64>> {
        self.conn()
            .query_row(
                "SELECT id FROM entries WHERE audio_sha256 = ?1 AND state IN (2, 3) ORDER BY state DESC LIMIT 1",
                params![audio_sha256],
                |r| r.get(0),
            )
            .optional()
    }

    /// Prefer measured local audio over untrusted claims for the same packet hash.
    pub fn audio_duration(&self, audio: &str) -> rusqlite::Result<Option<u64>> {
        self.conn().query_row("SELECT COALESCE(verified_duration_ms,duration_ms) FROM entries WHERE audio_sha256=?1 AND state!=4 ORDER BY (state=3 OR (state=2 AND verified_duration_ms IS NOT NULL)) DESC, duration_ms ASC LIMIT 1",[audio],|r|r.get::<_,i64>(0)).optional()?.map(|value|u64::try_from(value).map_err(|e|rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Integer,Box::new(e)))).transpose()
    }
    pub fn audio_verified(&self, audio: &str) -> rusqlite::Result<bool> {
        self.conn().prepare("SELECT 1 FROM entries WHERE audio_sha256=?1 AND (state=3 OR (state=2 AND verified_duration_ms IS NOT NULL))")?.exists([audio])
    }
    pub fn duration_verified(&self, id: i64) -> rusqlite::Result<bool> {
        self.conn().prepare("SELECT 1 FROM entries WHERE id=?1 AND (state=3 OR verified_duration_ms IS NOT NULL)")?.exists([id])
    }
    pub fn set_verified_duration(&self, id: i64, duration: u64) -> rusqlite::Result<()> {
        let duration = i64::try_from(duration)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        self.conn().execute(
            "UPDATE entries SET verified_duration_ms=?2 WHERE id=?1",
            params![id, duration],
        )?;
        Ok(())
    }

    pub fn id_for_info_hash(&self, info_hash: &str) -> rusqlite::Result<Option<i64>> {
        self.conn()
            .query_row(
                "SELECT id FROM entries WHERE info_hash = ?1",
                params![info_hash],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn peaks(&self, id: i64) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT peaks FROM entries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<bool> {
        let n = self
            .conn()
            .execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        Ok(n == 1)
    }

    /// Total size of the files in `state`.
    pub fn bytes_in_state(&self, state: State) -> rusqlite::Result<u64> {
        let used: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(size), 0) FROM entries WHERE state = ?1",
            params![state as i64],
            |r| r.get(0),
        )?;
        Ok(used as u64)
    }

    /// Downloaded tracks, least recently played first.
    pub fn eviction_candidates(&self) -> rusqlite::Result<Vec<(i64, String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, info_hash, size FROM entries WHERE state = 2 AND favorite_at IS NULL
             ORDER BY used_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
        })?;
        rows.collect()
    }

    /// Forgets search results that were never played: those older than a
    /// week, and the oldest beyond [`MAX_FOUND`], so that searching a lot
    /// never fills the disk.
    pub fn prune_found(&self) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let old = conn.execute(
            "DELETE FROM entries WHERE state = 0 AND favorite_at IS NULL AND seen_at < ?1",
            params![now() - FOUND_TTL_SECS],
        )?;
        let excess = conn.execute(
            "DELETE FROM entries WHERE state = 0 AND favorite_at IS NULL AND id NOT IN
                 (SELECT id FROM entries WHERE state = 0 ORDER BY seen_at DESC LIMIT ?1)",
            params![MAX_FOUND],
        )?;
        Ok(old + excess)
    }

    /// `(artist, genre, title, album)` of every track this node has the file
    /// of, for announcing its words.
    pub fn held_texts(&self) -> rusqlite::Result<Vec<[Option<String>; 4]>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(artist, album_artist), genre, title, album FROM entries
             WHERE state IN (2, 3) ORDER BY favorite_at IS NULL, used_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))?;
        rows.collect()
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> rusqlite::Result<bool> {
        let n = self.conn().execute(
            "UPDATE entries SET favorite_at = CASE WHEN ?2 THEN COALESCE(favorite_at, ?3) END
             WHERE id = ?1",
            params![id, favorite, now()],
        )?;
        Ok(n == 1)
    }

    /// Explicit favorites, most recently added first. Local sharing is independent.
    pub fn favorites(&self, limit: u32, offset: u32) -> rusqlite::Result<(Vec<Track>, u64)> {
        let conn = self.conn();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM entries WHERE (favorite_at IS NOT NULL) AND state != 4",
            [],
            |r| r.get(0),
        )?;
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM entries e
             WHERE (e.favorite_at IS NOT NULL) AND e.state != 4
             ORDER BY COALESCE(e.favorite_at, e.seen_at) DESC, e.id DESC LIMIT ?1 OFFSET ?2"
        );
        let me = self.me.clone();
        let mut stmt = conn.prepare(&sql)?;
        let tracks = stmt
            .query_map(params![limit, offset], |r| track_row(r, &me))?
            .map(|r| r.map(|row| row.track))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((tracks, total as u64))
    }

    /// What the listener's taste is made of: `(genre, artist, weight)` for
    /// every favorite, played or own track. Favorites count most.
    pub fn taste(&self) -> rusqlite::Result<Vec<(Option<String>, Option<String>, f64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT genre, COALESCE(artist, album_artist),
                 (CASE WHEN favorite_at IS NOT NULL THEN 5.0 ELSE 0.0 END)
                 + MIN(plays, 10) * 0.5
                 + (CASE WHEN state = 3 THEN 1.0 ELSE 0.0 END)
             FROM entries
             WHERE state != 4 AND (favorite_at IS NOT NULL OR plays > 0 OR state = 3)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Tracks that are neither favorites, own, nor already played, found by
    /// recent searches: candidates for recommendations.
    pub fn recommendation_candidates(&self, limit: u32) -> rusqlite::Result<Vec<TrackRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM entries e
             WHERE e.plays = 0 AND {DISCOVER_FILTER}
             GROUP BY e.audio_sha256
             ORDER BY e.seen_at DESC LIMIT ?1"
        );
        let me = self.me.clone();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], |r| track_row(r, &me))?;
        rows.collect()
    }

    /// Ids of the tracks in `states` matching `query` (see [`crate::fuzzy`]),
    /// best match first.
    /// Tracks in `states` matching `query`, in `order`. Popularity is how
    /// many computers are known to share a track, so that of two songs with
    /// the same name, the one most shared comes first.
    fn fuzzy_ids(
        &self,
        query: &Query,
        states: &str,
        order: SearchOrder,
    ) -> rusqlite::Result<Vec<i64>> {
        let key = format!("{states}:{}", query.cache_key());
        let (revision, high_id, cached) = {
            let conn = self.conn();
            let revision = search_revision(&conn)?;
            let cached = self
                .scores
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(revision, &key);
            let high_id: i64 =
                conn.query_row("SELECT COALESCE(MAX(id), 0) FROM entries", [], |r| r.get(0))?;
            (revision, high_id, cached)
        };
        let scores = if let Some(scores) = cached {
            scores
        } else {
            let mut matches = Vec::new();
            let mut after = 0i64;
            loop {
                // Release SQLite before CPU-heavy fuzzy scoring. A large
                // library must not monopolize writes/playback DB operations.
                let batch = {
                    let conn = self.conn();
                    let mut stmt = conn.prepare(&format!(
                        "SELECT id,title,artist,album,album_artist,genre FROM entries
                         WHERE state IN ({states}) AND id>?1 AND id<=?2 ORDER BY id LIMIT 256"
                    ))?;
                    let rows = stmt
                        .query_map(params![after, high_id], |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, Option<String>>(3)?,
                                r.get::<_, Option<String>>(4)?,
                                r.get::<_, Option<String>>(5)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                if batch.is_empty() {
                    break;
                }
                for (id, title, artist, album, album_artist, genre) in batch {
                    after = id;
                    let fields = Fields {
                        title: &title,
                        artist: artist.as_deref(),
                        album: album.as_deref(),
                        album_artist: album_artist.as_deref(),
                        genre: genre.as_deref(),
                    };
                    if let Some(score) = query.score(&fields) {
                        matches.push((score, id));
                    }
                }
            }
            let scores = Arc::new(matches);
            let conn = self.conn();
            // Any write, including a separate connection, invalidates scores.
            // Concurrently edited scans are useful but never cached as fresh.
            if search_revision(&conn)? == revision {
                self.scores
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(revision, key, scores.clone());
            }
            scores
        };
        let mut scored = Vec::with_capacity(scores.len());
        {
            let conn = self.conn();
            let mut popularity = conn.prepare_cached(&format!(
                "SELECT (SELECT COUNT(DISTINCT COALESCE(s.node,s.relay,s.addr))
                 FROM sources s JOIN entries copy ON copy.id=s.entry_id
                 WHERE copy.audio_sha256=e.audio_sha256 AND s.seen_at>=?2)
                 + (e.state IN (2,3)) FROM entries e WHERE e.id=?1 AND e.state IN ({states})"
            ))?;
            let fresh = now().saturating_sub(60);
            // Formerly the correlated source query ran for EVERY song,
            // including songs that did not match any word of the query.
            for &(score, id) in scores.iter() {
                if let Some(count) = popularity
                    .query_row(params![id, fresh], |r| r.get::<_, i64>(0))
                    .optional()?
                {
                    scored.push((score, count, id));
                }
            }
        }
        scored.sort_by(|a, b| {
            let by_score = b.0.total_cmp(&a.0);
            let by_popularity = b.1.cmp(&a.1);
            match order {
                SearchOrder::BestMatch => by_score.then(by_popularity),
                SearchOrder::Popular => by_popularity.then(by_score),
            }
            .then(a.2.cmp(&b.2))
        });
        Ok(scored.into_iter().map(|(_, _, id)| id).collect())
    }

    /// Entries of tracks this node can send, matching `query`, to answer
    /// another node's search: `(body, signature, info dictionary)`.
    pub fn search_held(&self, query: &str, limit: u32) -> rusqlite::Result<Vec<StoredEntry>> {
        let Some(query) = Query::new(query) else {
            return Ok(Vec::new());
        };
        let ids = self.fuzzy_ids(&query, "2, 3", SearchOrder::BestMatch)?;
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT body, sig, info FROM entries WHERE id = ?1 AND state IN (2,3)")?;
        ids.into_iter()
            .take(limit as usize)
            .filter_map(|id| {
                stmt.query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()
                    .transpose()
            })
            .collect()
    }

    /// Lists known tracks matching `query`, best match first, forgiving
    /// partial words and typos (all of them, own and played first, if
    /// empty). Tracks that failed validation are left out.
    pub fn search(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        order: SearchOrder,
    ) -> rusqlite::Result<(Vec<Track>, u64)> {
        self.search_filtered(query, limit, offset, order, false, false)
    }

    pub fn discover(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        order: SearchOrder,
    ) -> rusqlite::Result<(Vec<Track>, u64)> {
        self.search_filtered(query, limit, offset, order, true, false)
    }

    pub fn local_tracks(&self, limit: u32, offset: u32) -> rusqlite::Result<(Vec<Track>, u64)> {
        self.search_filtered("", limit, offset, SearchOrder::BestMatch, false, true)
    }

    fn search_filtered(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        order: SearchOrder,
        discovery: bool,
        local: bool,
    ) -> rusqlite::Result<(Vec<Track>, u64)> {
        let filter = if local {
            "e.state IN (2, 3)"
        } else if discovery {
            DISCOVER_FILTER
        } else {
            "e.state != 4"
        };
        let me = self.me.clone();
        let Some(fuzzy) = Query::new(query) else {
            let conn = self.conn();
            let count = if discovery {
                "COUNT(DISTINCT e.audio_sha256)"
            } else {
                "COUNT(*)"
            };
            let grouping = if discovery {
                "GROUP BY e.audio_sha256"
            } else {
                ""
            };
            let total: i64 = conn.query_row(
                &format!("SELECT {count} FROM entries e WHERE {filter}"),
                [],
                |r| r.get(0),
            )?;
            let sql = format!(
                "SELECT {TRACK_COLUMNS} FROM entries e WHERE {filter} {grouping}
                 ORDER BY e.state = 3 DESC, e.used_at DESC, e.created_at DESC, e.id DESC
                 LIMIT ?1 OFFSET ?2"
            );
            let mut stmt = conn.prepare(&sql)?;
            let tracks = stmt
                .query_map(params![limit, offset], |r| track_row(r, &me))?
                .map(|r| r.map(|row| row.track))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok((tracks, total as u64));
        };

        let mut ids = self.fuzzy_ids(&fuzzy, "0, 1, 2, 3", order)?;
        let conn = self.conn();
        if discovery {
            let mut eligible = conn.prepare(&format!(
                "SELECT 1 FROM entries e WHERE e.id = ?1 AND {filter}"
            ))?;
            let mut visible = Vec::new();
            let mut audio_hashes = std::collections::HashSet::new();
            for id in ids {
                if eligible.exists([id])? {
                    let audio: String = conn.query_row(
                        "SELECT audio_sha256 FROM entries WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?;
                    if audio_hashes.insert(audio) {
                        visible.push(id);
                    }
                }
            }
            ids = visible;
        }
        let total = ids.len() as u64;
        let sql = format!("SELECT {TRACK_COLUMNS} FROM entries e WHERE e.id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let tracks = ids
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .filter_map(|id| {
                stmt.query_row(params![id], |r| track_row(r, &me))
                    .optional()
                    .map(|row| row.map(|row| row.track))
                    .transpose()
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((tracks, total))
    }
}

pub fn format_name(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Flac => "flac",
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Mp4 => "mp4",
        AudioFormat::Ogg => "ogg",
        AudioFormat::Wav => "wav",
    }
}

fn parse_format(s: &str, column: usize) -> rusqlite::Result<AudioFormat> {
    AudioFormat::ALL
        .into_iter()
        .find(|f| format_name(*f) == s)
        .ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(column, "format".into(), rusqlite::types::Type::Text)
        })
}

/// How a publisher is shown: their chosen name, if any, and the start of
/// their key, which is what actually identifies them.
pub fn uploader_label(publisher: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{name} · {}", fingerprint(publisher)),
        None => fingerprint(publisher).to_string(),
    }
}

fn track_row(r: &Row<'_>, me: &str) -> rusqlite::Result<TrackRow> {
    let publisher: String = r.get(16)?;
    let name: Option<String> = r.get(17)?;
    let state = State::from_i64(r.get(19)?);
    Ok(TrackRow {
        track: Track {
            id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            album: r.get(3)?,
            album_artist: r.get(4)?,
            track_number: r.get(5)?,
            year: r.get(6)?,
            genre: r.get(7)?,
            duration_ms: r.get::<_, i64>(8)? as u64,
            format: parse_format(&r.get::<_, String>(9)?, 9)?,
            codec: r.get(10)?,
            sample_rate: r.get(11)?,
            channels: r.get(12)?,
            size: r.get::<_, i64>(13)? as u64,
            sha256: r.get(14)?,
            has_cover: r.get(15)?,
            uploader: uploader_label(&publisher, name.as_deref()),
            uploaded_at: r.get(18)?,
            mine: state == State::Own && publisher == me,
            local: state.has_file(),
            favorite: r.get(21)?,
            sources: r.get::<_, i64>(22)?.clamp(0, i64::from(u32::MAX)) as u32,
        },
        state,
        info_hash: r.get(20)?,
    })
}

fn torrent_row(r: &Row<'_>) -> rusqlite::Result<TorrentRow> {
    Ok(TorrentRow {
        id: r.get(0)?,
        info_hash: r.get(1)?,
        info: r.get(2)?,
        format: parse_format(&r.get::<_, String>(3)?, 3)?,
        state: State::from_i64(r.get(4)?),
        sha256: r.get(5)?,
        audio_sha256: r.get(6)?,
        duration_ms: r.get::<_, i64>(7)? as u64,
    })
}

/// Turns free-form search input into an FTS5 query of quoted prefix terms,
/// all of which must match. Returns `None` for an empty query.
pub fn fts_query(input: &str) -> Option<String> {
    let input: String = crate::keywords::strip_extensions(input)
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '\'' | '-' | '&'))
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .take(MAX_QUERY_TERMS)
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_cache_persists_expires_and_backs_off_without_private_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = Db::open(&path, "me").unwrap();
        let public: SocketAddr = "8.8.8.8:47391".parse().unwrap();
        let ipv6: SocketAddr = "[2606:4700::1111]:47391".parse().unwrap();
        for addr in [
            public,
            ipv6,
            "127.0.0.1:47391".parse().unwrap(),
            "192.168.1.1:47391".parse().unwrap(),
            "8.8.8.8:80".parse().unwrap(),
        ] {
            db.remember_peer(addr).unwrap();
        }
        assert_eq!(db.known_peers(false).unwrap().len(), 2);
        assert_eq!(db.known_peers(true).unwrap(), vec![public]);
        drop(db);
        let db = Db::open(&path, "me").unwrap();
        assert_eq!(db.known_peers(true).unwrap(), vec![public]);
        db.peer_failed(public).unwrap();
        assert!(db.known_peers(true).unwrap().is_empty());
        let retry: i64 = db
            .conn()
            .query_row(
                "SELECT retry_at FROM known_peers WHERE addr=?1",
                [public.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!((119..=120).contains(&(retry - now())));
        for _ in 0..10 {
            db.peer_failed(public).unwrap();
        }
        let retry: i64 = db
            .conn()
            .query_row(
                "SELECT retry_at FROM known_peers WHERE addr=?1",
                [public.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!((1799..=1800).contains(&(retry - now())));
        db.remember_peer(public).unwrap();
        assert_eq!(db.known_peers(true).unwrap(), vec![public]);
        db.conn()
            .execute(
                "UPDATE known_peers SET seen_at=?1",
                [now() - 8 * 24 * 60 * 60],
            )
            .unwrap();
        assert!(db.known_peers(false).unwrap().is_empty());
        // Revalidate on read, even if an old or modified database contains a
        // private destination or a timestamp from before a clock correction.
        db.conn()
            .execute(
                "INSERT INTO known_peers(addr, seen_at) VALUES ('127.0.0.1:47391', ?1)",
                [now()],
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE known_peers SET seen_at=?1 WHERE addr=?2",
                params![now() + 3600, public.to_string()],
            )
            .unwrap();
        assert!(db.known_peers(false).unwrap().is_empty());
        for port in 2000..2200 {
            db.remember_peer(SocketAddr::from(([8, 8, 8, 8], port)))
                .unwrap();
        }
        let count: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM known_peers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 128);
    }

    #[test]
    fn discovery_filters_local_audio_before_pagination_and_stars_are_independent() {
        use crate::{
            entry::{Entry, TrackMeta},
            identity::Identity,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = Db::open(&path, "me").unwrap();
        let entry = |seed: char, audio: char| {
            let meta = TrackMeta {
                title: "Numb".into(),
                artist: Some("Linkin Park".into()),
                album: None,
                album_artist: None,
                track_number: None,
                year: None,
                genre: None,
                duration_ms: 1000,
                format: AudioFormat::Flac,
                codec: "flac".into(),
                sample_rate: 44100,
                channels: 2,
                size: 1000,
                sha256: seed.to_string().repeat(64),
                audio_sha256: audio.to_string().repeat(64),
            };
            Entry::create(
                &Identity::generate(),
                None,
                100,
                format!("info {seed}").into_bytes(),
                meta,
                None,
            )
        };
        let own = db.insert_entry(&entry('a', 'a'), State::Own).unwrap();
        let held = db.insert_entry(&entry('b', 'b'), State::Held).unwrap();
        db.insert_entry(&entry('c', 'a'), State::Found).unwrap(); // retagged duplicate
        let available = db.insert_entry(&entry('d', 'd'), State::Found).unwrap();
        db.add_source(
            available,
            Source {
                addr: "127.0.0.1:9991".parse().unwrap(),
                node: Some("127.0.0.1:9992".parse().unwrap()),
                relay: None,
            },
        )
        .unwrap();
        let starred = db.insert_entry(&entry('e', 'e'), State::Found).unwrap();
        db.set_favorite(starred, true).unwrap();
        assert_eq!(
            db.favorites(20, 0).unwrap().1,
            1,
            "sharing alone must not create a star"
        );
        for query in ["", "numb"] {
            let (tracks, total) = db.discover(query, 1, 0, SearchOrder::BestMatch).unwrap();
            assert_eq!(total, 1);
            assert_eq!(tracks[0].id, available);
            assert!(db
                .discover(query, 1, 1, SearchOrder::BestMatch)
                .unwrap()
                .0
                .is_empty());
        }
        db.set_favorite(own, true).unwrap();
        db.set_favorite(own, false).unwrap();
        db.set_favorite(held, true).unwrap();
        db.set_favorite(held, false).unwrap();
        assert_eq!(db.local_tracks(20, 0).unwrap().1, 2);
        assert_eq!(db.search_held("numb", 20).unwrap().len(), 2);
        drop(db);
        let reopened = Db::open(&path, "me").unwrap();
        assert_eq!(
            reopened.favorites(20, 0).unwrap().1,
            1,
            "restart must not re-star local songs"
        );
    }

    #[test]
    fn fts_queries_are_quoted() {
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query("   "), None);
        assert_eq!(fts_query("daft punk"), Some("\"daft\"* \"punk\"*".into()));
        // FTS5 operators and column filters are neutralized
        assert_eq!(
            fts_query("title:x OR \"y NEAR(z"),
            Some("\"titlex\"* \"OR\"* \"y\"* \"NEARz\"*".into())
        );
        assert_eq!(fts_query("*) --"), Some("\"--\"*".into()));
        assert_eq!(fts_query("Beyoncé"), Some("\"Beyoncé\"*".into()));
    }

    #[test]
    fn covers_follow_newer_signed_entries_from_the_same_publisher() {
        use crate::{
            entry::{Entry, TrackMeta},
            identity::Identity,
        };
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db"), "me").unwrap();
        let publisher = Identity::generate();
        let entry = |created_at, cover: Option<String>| {
            let meta = TrackMeta {
                title: "Song".into(),
                artist: None,
                album: None,
                album_artist: None,
                track_number: None,
                year: None,
                genre: None,
                duration_ms: 1000,
                format: AudioFormat::Flac,
                codec: "flac".into(),
                sample_rate: 44_100,
                channels: 2,
                size: 1000,
                sha256: "0".repeat(64),
                audio_sha256: "1".repeat(64),
            };
            Entry::create(&publisher, None, created_at, b"info".to_vec(), meta, cover)
        };
        let cover = |id| db.cover_state(id).unwrap().unwrap().sha256;

        let id = db.insert_entry(&entry(100, None), State::Found).unwrap();
        assert_eq!(cover(id), None);
        // Shared before covers were passed on: the cover arrives later
        db.insert_entry(&entry(200, Some("a".repeat(64))), State::Found)
            .unwrap();
        assert_eq!(cover(id), Some("a".repeat(64)));
        db.set_has_cover(id).unwrap();
        // A newer signed version invalidates a cached picture.
        db.insert_entry(&entry(300, Some("b".repeat(64))), State::Found)
            .unwrap();
        assert_eq!(cover(id), Some("b".repeat(64)));
        assert!(!db.cover_state(id).unwrap().unwrap().here);
        db.insert_entry(&entry(200, Some("a".repeat(64))), State::Found)
            .unwrap();
        assert_eq!(cover(id), Some("b".repeat(64)));
        let foreign = Entry::create(
            &Identity::generate(),
            None,
            400,
            b"info".to_vec(),
            entry(300, None).track,
            Some("c".repeat(64)),
        );
        db.insert_entry(&foreign, State::Found).unwrap();
        assert_eq!(cover(id), Some("b".repeat(64)));
    }

    #[test]
    fn the_most_shared_of_equal_matches_comes_first() {
        use crate::{
            entry::{Entry, TrackMeta},
            identity::Identity,
        };
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db"), "me").unwrap();
        let entry = |title: &str, audio: char| {
            let meta = TrackMeta {
                title: title.into(),
                artist: Some("Linkin Park".into()),
                album: None,
                album_artist: None,
                track_number: None,
                year: None,
                genre: None,
                duration_ms: 1000,
                format: AudioFormat::Flac,
                codec: "flac".into(),
                sample_rate: 44_100,
                channels: 2,
                size: 1000,
                sha256: audio.to_string().repeat(64),
                audio_sha256: audio.to_string().repeat(64),
            };
            let info = format!("info {audio}").into_bytes();
            Entry::create(&Identity::generate(), None, 100, info, meta, None)
        };
        let rare = db.insert_entry(&entry("Numb", 'a'), State::Found).unwrap();
        let popular = db.insert_entry(&entry("Numb", 'b'), State::Found).unwrap();
        let other = db
            .insert_entry(&entry("Numb Encore", 'c'), State::Found)
            .unwrap();
        for (id, sources) in [(rare, 1), (popular, 3), (other, 5)] {
            for n in 0..sources {
                let source = Source {
                    addr: format!("192.0.2.{n}:6881").parse().unwrap(),
                    relay: None,
                    node: None,
                };
                db.add_source(id, source).unwrap();
            }
        }
        let ids = |order| -> Vec<i64> {
            let (tracks, _) = db.search("numb", 10, 0, order).unwrap();
            assert_eq!(tracks.iter().find(|t| t.id == popular).unwrap().sources, 3);
            tracks.into_iter().map(|t| t.id).collect()
        };
        // Exact title matches first, the more shared of them on top
        assert_eq!(ids(SearchOrder::BestMatch), [popular, rare, other]);
        assert_eq!(ids(SearchOrder::Popular), [other, popular, rare]);
    }
}

#[cfg(test)]
mod search_scaling_tests {
    use super::*;

    fn catalog(count: usize) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("catalog.sqlite"), "test").unwrap();
        {
            let mut conn = db.conn();
            let transaction = conn.transaction().unwrap();
            {
                let mut insert = transaction.prepare(
                    "INSERT INTO entries(info_hash,publisher,body,sig,info,created_at,title,artist,
                    duration_ms,format,codec,sample_rate,channels,size,sha256,audio_sha256,state,seen_at)
                    VALUES(?1,'test',X'00',X'00',X'00',0,?2,'Orchestra',1000,'flac','flac',44100,2,1000,?1,?1,3,0)"
                ).unwrap();
                for n in 0..count {
                    let title = if n == 0 {
                        "Needle".into()
                    } else {
                        format!("Ambient recording {n}")
                    };
                    insert.execute(params![format!("{n:064x}"), title]).unwrap();
                }
            }
            transaction.commit().unwrap();
        }
        (dir, db)
    }

    #[test]
    fn scores_are_reused_but_edits_and_external_writes_invalidate_them() {
        let (_dir, db) = catalog(300);
        let query = Query::new("needle").unwrap();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![1]
        );
        let first = db.scores.lock().unwrap().entries.back().unwrap().2.clone();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![1]
        );
        assert!(Arc::ptr_eq(
            &first,
            &db.scores.lock().unwrap().entries.back().unwrap().2
        ));
        db.conn()
            .execute("UPDATE entries SET title='Unrelated' WHERE id=1", [])
            .unwrap();
        assert!(db
            .fuzzy_ids(&query, "2,3", SearchOrder::BestMatch)
            .unwrap()
            .is_empty());
        let other = Connection::open(_dir.path().join("catalog.sqlite")).unwrap();
        other
            .execute("UPDATE entries SET title='Needle' WHERE id=2", [])
            .unwrap();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![2]
        );
        db.conn()
            .execute("UPDATE entries SET state=4 WHERE id=2", [])
            .unwrap();
        assert!(db.search_held("needle", 50).unwrap().is_empty());
    }

    #[test]
    fn score_cache_has_a_hard_byte_and_entry_budget() {
        let mut cache = ScoreCache::default();
        for n in 0..100 {
            cache.insert((0, 0), n.to_string(), Arc::new(vec![(1.0, 1); 40_000]));
            assert!(cache.bytes <= SCORE_CACHE_BYTES);
            assert!(cache.entries.len() <= 4);
        }
        let before = cache.bytes;
        cache.insert(
            (0, 0),
            "oversized".into(),
            Arc::new(vec![(1.0, 1); SCORE_CACHE_BYTES / 16 + 1]),
        );
        assert_eq!(cache.bytes, before);
        assert!(cache.get((0, 0), "oversized").is_none());
        cache.insert(
            (0, 0),
            "reserved".into(),
            Arc::new(Vec::with_capacity(SCORE_CACHE_BYTES)),
        );
        assert!(cache.get((0, 0), "reserved").is_none());
        assert_eq!(cache.bytes, before);
    }

    #[test]
    fn cached_fuzzy_order_and_live_peer_popularity_are_preserved() {
        let (_dir, db) = catalog(300);
        db.conn()
            .execute("UPDATE entries SET title='Needle Encore' WHERE id=2", [])
            .unwrap();
        db.conn()
            .execute("UPDATE entries SET title='Needle' WHERE id=3", [])
            .unwrap();
        let query = Query::new("needle").unwrap();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![1, 3, 2]
        );
        db.add_source(
            3,
            Source {
                addr: "127.0.0.1:1234".parse().unwrap(),
                relay: None,
                node: Some("127.0.0.1:4321".parse().unwrap()),
            },
        )
        .unwrap();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![3, 1, 2]
        );
        db.conn()
            .execute("UPDATE sources SET seen_at=0", [])
            .unwrap();
        assert_eq!(
            db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
            vec![1, 3, 2]
        );
    }

    #[test]
    #[ignore = "Local 20,000-track timing probe; run explicitly with --ignored --nocapture"]
    fn large_catalog_repeated_search_probe() {
        let (_dir, db) = catalog(20_000);
        let query = Query::new("needle").unwrap();
        let start = std::time::Instant::now();
        let first = db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap();
        let cold = start.elapsed();
        assert_eq!(first, vec![1]);
        let start = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(
                db.fuzzy_ids(&query, "2,3", SearchOrder::BestMatch).unwrap(),
                first
            );
        }
        eprintln!(
            "20,000 local tracks: cold={cold:?}; 100 repeated queries={:?}; cache={} bytes",
            start.elapsed(),
            db.scores.lock().unwrap().bytes
        );
    }
}
