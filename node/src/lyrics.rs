// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics are a signed attachment, not part of the torrent. Old peers can
//! still download the same audio; updated peers also keep and forward lyrics.
use crate::{db::now, entry::Entry, AppState};
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    time::Duration,
};

pub const PATH: &str = "/p2p/v1/lyrics";
pub const MAX_TEXT: usize = 256 * 1024;
pub const MAX_WIRE: usize = MAX_TEXT * 4 + 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    body: String,
    signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u8,
    info_hash: String,
    audio_sha256: String,
    created_at: i64,
    text: String,
}

pub(crate) fn valid_text(text: &str) -> bool {
    hopea_common::lyrics::validate(text, None).is_ok()
}

fn verify(bytes: &[u8], entry: &Entry) -> anyhow::Result<Body> {
    anyhow::ensure!(bytes.len() <= MAX_WIRE, "lyrics document too large");
    let document: Document = serde_json::from_slice(bytes)?;
    anyhow::ensure!(
        document.signature.len() == 128 && document.body.len() <= MAX_TEXT * 2 + 2048,
        "invalid lyrics document"
    );
    let key: [u8; 32] = hex::decode(&entry.publisher)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid publisher"))?;
    let signature = Signature::from_slice(&hex::decode(document.signature)?)?;
    VerifyingKey::from_bytes(&key)?.verify_strict(document.body.as_bytes(), &signature)?;
    let body: Body = serde_json::from_str(&document.body)?;
    anyhow::ensure!(
        body.version == 1
            && body.info_hash == entry.info_hash
            && body.audio_sha256 == entry.track.audio_sha256,
        "lyrics belong to another track"
    );
    anyhow::ensure!(
        (1_577_836_800..=now().saturating_add(86_400)).contains(&body.created_at)
            && hopea_common::lyrics::validate(&body.text, Some(entry.track.duration_ms)).is_ok(),
        "invalid lyrics"
    );
    Ok(body)
}

async fn entry(state: &AppState, id: i64) -> anyhow::Result<Entry> {
    let db = state.db.clone();
    let (body, sig) = tokio::task::spawn_blocking(move || db.entry_bytes(id))
        .await??
        .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
    Ok(Entry::from_stored(
        body,
        sig,
        None,
        &state.config.limits,
        now(),
    )?)
}

fn read(state: &AppState, hash: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(state.storage.lyrics_path(hash))
        .ok()?
        .take(MAX_WIRE as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_WIRE).then_some(bytes)
}

fn store(state: &AppState, hash: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let path = state.storage.lyrics_path(hash);
    crate::fsx::create_private_dir(path.parent().unwrap())?;
    let mut file = tempfile::NamedTempFile::new_in(state.storage.tmp_dir())?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

pub async fn document(state: &AppState, hash: &str) -> Option<Vec<u8>> {
    let state = state.clone();
    let hash = hash.to_string();
    tokio::task::spawn_blocking(move || shared_document(&state, &hash))
        .await
        .ok()
        .flatten()
}

pub async fn accept(state: &AppState, id: i64, bytes: Vec<u8>) -> anyhow::Result<()> {
    let _guard = state.lyrics_work.lock().await;
    let entry = entry(state, id).await?;
    let state = state.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let body = verify(&bytes, &entry)?;
        let old = read(&state, &entry.info_hash).and_then(|b| verify(&b, &entry).ok());
        if old.is_none_or(|old| old.created_at < body.created_at) {
            store(&state, &entry.info_hash, &bytes)?;
        }
        Ok(())
    })
    .await?
}

pub async fn publish(state: &AppState, id: i64, text: String) -> anyhow::Result<()> {
    anyhow::ensure!(
        valid_text(&text),
        "lyrics exceed limits or contain invalid control characters"
    );
    let _guard = state.lyrics_work.lock().await;
    let entry = entry(state, id).await?;
    hopea_common::lyrics::validate(&text, Some(entry.track.duration_ms))?;
    anyhow::ensure!(
        entry.publisher == state.identity.public_hex(),
        "only the original publisher can sign lyrics"
    );
    let state = state.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let previous = read(&state, &entry.info_hash).and_then(|b| verify(&b, &entry).ok());
        if previous.as_ref().is_some_and(|p| p.text == text) {
            return Ok(());
        }
        let created_at = now().max(previous.map_or(0, |p| p.created_at.saturating_add(1)));
        anyhow::ensure!(
            created_at <= now().saturating_add(86_400),
            "too many lyrics revisions"
        );
        let body = serde_json::to_string(&Body {
            version: 1,
            info_hash: entry.info_hash.clone(),
            audio_sha256: entry.track.audio_sha256,
            created_at,
            text,
        })?;
        let document = Document {
            signature: hex::encode(state.identity.sign(body.as_bytes())),
            body,
        };
        store(&state, &entry.info_hash, &serde_json::to_vec(&document)?)
    })
    .await??;
    Ok(())
}

pub async fn fetch(state: &AppState, id: i64) -> anyhow::Result<Option<String>> {
    let _guard = state.lyrics_work.lock().await;
    let entry = entry(state, id).await?;
    let state2 = state.clone();
    let saved_entry = entry.clone();
    let cached = tokio::task::spawn_blocking(move || {
        read(&state2, &saved_entry.info_hash).and_then(|bytes| verify(&bytes, &saved_entry).ok())
    })
    .await?;
    if entry.publisher == state.identity.public_hex() {
        return Ok(cached.map(|body| body.text));
    }
    let db = state.db.clone();
    let sources = tokio::task::spawn_blocking(move || db.sources(id)).await??;
    let mut nodes = std::collections::HashSet::new();
    let download = async {
        for node in sources
            .into_iter()
            .filter_map(|s| s.relay.or(s.node))
            .filter(|node| nodes.insert(*node))
            .take(8)
        {
            let response = state
                .http
                .get(format!("http://{node}{PATH}?info_hash={}", entry.info_hash))
                .timeout(Duration::from_secs(3))
                .send()
                .await;
            let Ok(response) = response else {
                continue;
            };
            let Ok(document) = crate::reach::read_json::<Document>(response, MAX_WIRE).await else {
                continue;
            };
            let bytes = serde_json::to_vec(&document)?;
            let Ok(body) = verify(&bytes, &entry) else {
                continue;
            };
            if cached
                .as_ref()
                .is_some_and(|old| old.created_at >= body.created_at)
            {
                continue;
            }
            let state2 = state.clone();
            let hash = entry.info_hash.clone();
            tokio::task::spawn_blocking(move || store(&state2, &hash, &bytes)).await??;
            return anyhow::Ok(Some(body.text));
        }
        Ok(None)
    };
    Ok(tokio::time::timeout(Duration::from_secs(8), download)
        .await
        .unwrap_or(Ok(None))?
        .or_else(|| cached.map(|body| body.text)))
}

#[derive(Deserialize)]
pub struct LyricsQuery {
    info_hash: String,
}
pub async fn serve(
    State(state): State<AppState>,
    Query(query): Query<LyricsQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], Vec<u8>), StatusCode> {
    if !crate::torrent_info::is_info_hash(&query.info_hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let bytes = tokio::task::spawn_blocking(move || shared_document(&state, &query.info_hash))
        .await
        .ok()
        .flatten()
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes))
}

/// Read only a previously verified attachment; never fetch from a public handler.
pub(crate) fn cached(state: &AppState, id: i64) -> Option<String> {
    let (body, sig) = state.db.entry_bytes(id).ok()??;
    let entry = Entry::from_stored(body, sig, None, &state.config.limits, now()).ok()?;
    verify(&read(state, &entry.info_hash)?, &entry)
        .ok()
        .map(|b| b.text)
}

fn shared_document(state: &AppState, hash: &str) -> Option<Vec<u8>> {
    let id = state.db.id_for_info_hash(hash).ok()??;
    let row = state.db.torrent(id).ok()??;
    if !row.state.has_file() || !state.db.duration_verified(id).ok()? {
        return None;
    }
    let (body, sig) = state.db.entry_bytes(id).ok()??;
    let entry = Entry::from_stored(body, sig, None, &state.config.limits, now()).ok()?;
    let bytes = read(state, hash)?;
    let lyrics = verify(&bytes, &entry).ok()?;
    hopea_common::lyrics::validate(&lyrics.text, Some(row.duration_ms)).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod security_tests {
    use super::*;
    #[tokio::test]
    async fn even_correctly_signed_poisoned_lyrics_cannot_be_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::config::Config::isolated(dir.path().join("data")),
            &crate::auth::generate_token(),
        )
        .await
        .unwrap();
        let input =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
        let ingested = crate::ingest::ingest(
            &input,
            "Artist - Song.flac",
            &state.storage.tmp_dir(),
            &state.config.limits,
        )
        .unwrap();
        let id = crate::library::share(&state, ingested).await.unwrap().id;
        let entry = entry(&state, id).await.unwrap();
        let end = entry.track.duration_ms + 1;
        let after_end = format!(
            "[{:02}:{:02}.{:03}]Too late",
            end / 60_000,
            (end / 1000) % 60,
            end % 1000
        );
        for (text, audio) in [
            ("[-01:00]Bad", entry.track.audio_sha256.clone()),
            (after_end.as_str(), entry.track.audio_sha256.clone()),
            ("[00:00.00]Other song", "f".repeat(64)),
        ] {
            let body = serde_json::to_string(&Body {
                version: 1,
                info_hash: entry.info_hash.clone(),
                audio_sha256: audio,
                created_at: now(),
                text: text.into(),
            })
            .unwrap();
            let doc = serde_json::to_vec(&Document {
                signature: hex::encode(state.identity.sign(body.as_bytes())),
                body,
            })
            .unwrap();
            assert!(accept(&state, id, doc.clone()).await.is_err());
            store(&state, &entry.info_hash, &doc).unwrap();
            assert!(document(&state, &entry.info_hash).await.is_none());
            assert_eq!(
                serve(
                    State(state.clone()),
                    Query(LyricsQuery {
                        info_hash: entry.info_hash.clone()
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
    }
}
