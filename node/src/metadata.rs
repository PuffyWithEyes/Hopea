// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Alternatives belong to audio packets, never to mutable tags or filenames.
//! Counts are observations of distinct signed respondents, not supplied ratings.
use crate::{db::now, AppState};
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    Json,
};
use ed25519_dalek::{Signature, VerifyingKey};
use hopea_common::api::MetadataVariant;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, net::SocketAddr, time::Duration};

pub const PATH: &str = "/p2p/v1/metadata";
pub const DATA_PATH: &str = "/p2p/v1/metadata/data";
pub const MAX_DATA: usize = 1024 * 1024;
const MAX_VARIANTS: usize = 16;
const MAX_STORE: i64 = 128 * 1024 * 1024;
const MAX_MANIFEST: usize = 16 * 1024;

fn hash_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn content_hash(audio: &str, kind: &str, data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"hopea-metadata-v1\0");
    h.update(audio);
    h.update(kind);
    h.update(data);
    hex::encode(h.finalize())
}
fn valid(kind: &str, data: &[u8]) -> bool {
    if data.len() > MAX_DATA {
        return false;
    }
    match kind {
        "cover" => crate::ingest::reencode_cover(data).is_some(),
        "lyrics" => std::str::from_utf8(data)
            .ok()
            .is_some_and(crate::lyrics::valid_text),
        _ => false,
    }
}
fn audio(state: &AppState, id: i64) -> anyhow::Result<String> {
    Ok(state
        .db
        .torrent(id)?
        .ok_or_else(|| anyhow::anyhow!("unknown track"))?
        .audio_sha256)
}
fn put(
    state: &AppState,
    audio: &str,
    kind: &str,
    data: &[u8],
    select: bool,
    contributed: bool,
) -> anyhow::Result<String> {
    anyhow::ensure!(hash_valid(audio) && valid(kind, data), "invalid attachment");
    if kind == "lyrics" {
        let duration = state
            .db
            .audio_duration(audio)?
            .ok_or_else(|| anyhow::anyhow!("unknown audio"))?;
        hopea_common::lyrics::validate(std::str::from_utf8(data)?, Some(duration))?;
    }
    let hash = content_hash(audio, kind, data);
    let mut conn = state.db.conn();
    let tx = conn.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata_variants WHERE audio=?1 AND hash=?2)",
        params![audio, hash],
        |r| r.get(0),
    )?;
    if !exists {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM metadata_variants WHERE audio=?1",
            [audio],
            |r| r.get(0),
        )?;
        let per_audio: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(data)),0) FROM metadata_variants WHERE audio=?1",
            [audio],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            per_audio.saturating_add(data.len() as i64) <= 4 * 1024 * 1024,
            "track attachment budget exceeded"
        );
        let bytes: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(data)),0) FROM metadata_variants",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(
            count < MAX_VARIANTS as i64 && bytes.saturating_add(data.len() as i64) <= MAX_STORE,
            "attachment storage limit reached"
        );
        tx.execute(
            "INSERT INTO metadata_variants(audio,hash,kind,data) VALUES(?1,?2,?3,?4)",
            params![audio, hash, kind, data],
        )?;
    }
    if contributed {
        tx.execute(
            "UPDATE metadata_variants SET contributed=1 WHERE audio=?1 AND hash=?2",
            params![audio, hash],
        )?;
    }
    if select {
        tx.execute(
            "UPDATE metadata_variants SET selected=(hash=?3) WHERE audio=?1 AND kind=?2",
            params![audio, kind, hash],
        )?;
    }
    tx.commit()?;
    Ok(hash)
}
// Lazy migration of attachments held by older installations. This only reads
// local bounded files; a public request never triggers another network request.
fn seed_local(state: &AppState, audio: &str) -> anyhow::Result<()> {
    use std::io::Read;
    let Some(id) = state.db.own_track_with_audio(audio)? else {
        return Ok(());
    };
    let variants = list_local(state, audio)?;
    if !variants.iter().any(|v| v.kind == "cover") {
        if let Some(row) = state.db.torrent(id)? {
            if let Ok(file) = std::fs::File::open(state.storage.cover_path(&row.info_hash)) {
                let mut data = Vec::new();
                file.take(MAX_DATA as u64 + 1).read_to_end(&mut data)?;
                let _ = put(state, audio, "cover", &data, true, true);
            }
        }
    }
    if !variants.iter().any(|v| v.kind == "lyrics") {
        if let Some(text) = crate::lyrics::cached(state, id) {
            let _ = put(state, audio, "lyrics", text.as_bytes(), true, true);
        }
    }
    Ok(())
}
fn list_local(state: &AppState, audio: &str) -> anyhow::Result<Vec<MetadataVariant>> {
    let conn = state.db.conn();
    let mut q=conn.prepare("SELECT hash,kind,length(data),selected,(SELECT COUNT(DISTINCT peer) FROM metadata_sources s WHERE s.audio=v.audio AND s.hash=v.hash AND s.seen_at>=?2) + (selected OR contributed) FROM metadata_variants v WHERE audio=?1 ORDER BY kind,hash LIMIT 16")?;
    let mut result = q
        .query_map(params![audio, now().saturating_sub(60)], |r| {
            Ok(MetadataVariant {
                hash: r.get(0)?,
                kind: r.get(1)?,
                size: r.get(2)?,
                selected: r.get(3)?,
                peers: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(q);
    drop(conn);
    result.retain(|v| v.kind != "lyrics" || bytes(state, audio, &v.hash).ok().flatten().is_some());
    if !state.db.audio_verified(audio)? {
        for variant in &mut result {
            if variant.selected || variant.peers > 0 {
                // Recompute without counting this node until its audio is validated.
                variant.peers=state.db.conn().query_row("SELECT COUNT(DISTINCT peer) FROM metadata_sources WHERE audio=?1 AND hash=?2 AND seen_at>=?3",params![audio,variant.hash,now().saturating_sub(60)],|r|r.get(0))?;
            }
        }
    }
    Ok(result)
}
fn bytes(state: &AppState, audio: &str, hash: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let row: Option<(String, Vec<u8>)> = state
        .db
        .conn()
        .query_row(
            "SELECT kind,data FROM metadata_variants WHERE audio=?1 AND hash=?2",
            params![audio, hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((kind, data)) = row else {
        return Ok(None);
    };
    if content_hash(audio, &kind, &data) != hash {
        return Ok(None);
    }
    if kind == "lyrics" {
        let duration = state
            .db
            .audio_duration(audio)?
            .ok_or_else(|| anyhow::anyhow!("unknown audio"))?;
        if std::str::from_utf8(&data)
            .ok()
            .is_none_or(|text| hopea_common::lyrics::validate(text, Some(duration)).is_err())
        {
            return Ok(None);
        }
    }
    Ok(Some(data))
}

pub async fn publish(
    state: &AppState,
    id: i64,
    kind: &str,
    data: Vec<u8>,
    select: bool,
) -> anyhow::Result<String> {
    let state = state.clone();
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        put(&state, &audio(&state, id)?, &kind, &data, select, true)
    })
    .await?
}
pub async fn selected(state: &AppState, id: i64, kind: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let state = state.clone();
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        let audio = audio(&state, id)?;
        let hash: Option<String> = state
            .db
            .conn()
            .query_row(
                "SELECT hash FROM metadata_variants WHERE audio=?1 AND kind=?2 AND selected=1",
                params![audio, kind],
                |r| r.get(0),
            )
            .optional()?;
        match hash {
            Some(hash) => bytes(&state, &audio, &hash),
            None => Ok(None),
        }
    })
    .await?
}
pub async fn data(state: &AppState, id: i64, hash: String) -> anyhow::Result<Option<Vec<u8>>> {
    anyhow::ensure!(hash_valid(&hash), "invalid hash");
    let state = state.clone();
    tokio::task::spawn_blocking(move || bytes(&state, &audio(&state, id)?, &hash)).await?
}
pub async fn select(state: &AppState, id: i64, hash: String) -> anyhow::Result<()> {
    anyhow::ensure!(hash_valid(&hash), "invalid hash");
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let audio = audio(&state, id)?;
        anyhow::ensure!(
            bytes(&state, &audio, &hash)?.is_some(),
            "invalid or missing attachment"
        );
        let mut conn = state.db.conn();
        let tx = conn.transaction()?;
        let kind: String = tx.query_row(
            "SELECT kind FROM metadata_variants WHERE audio=?1 AND hash=?2",
            params![audio, hash],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE metadata_variants SET selected=(hash=?3) WHERE audio=?1 AND kind=?2",
            params![audio, kind, hash],
        )?;
        tx.commit()?;
        anyhow::Ok(())
    })
    .await?
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    body: String,
    peer: String,
    signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    audio: String,
    nonce: String,
    variants: Vec<MetadataVariant>,
    proofs: Vec<String>,
}
#[derive(Deserialize)]
pub struct ManifestQuery {
    audio: String,
    nonce: String,
}
pub async fn serve(
    State(state): State<AppState>,
    Query(q): Query<ManifestQuery>,
) -> Result<Json<Envelope>, StatusCode> {
    if !hash_valid(&q.audio) || !hash_valid(&q.nonce) {
        return Err(StatusCode::BAD_REQUEST);
    }
    tokio::task::spawn_blocking(move || {
        seed_local(&state, &q.audio).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut variants =
            list_local(&state, &q.audio).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let audio_key=q.audio.clone();
        let offered:HashSet<String>={
            let conn=state.db.conn();let mut q=conn.prepare("SELECT hash FROM metadata_variants WHERE audio=?1 AND (selected OR contributed)").map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
            let hashes=q.query_map([&audio_key],|r|r.get(0)).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?.collect::<Result<HashSet<String>,_>>().map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
            hashes
        };
        let verified = state.db.audio_verified(&q.audio).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
        variants.retain(|v| verified && offered.contains(&v.hash));
        // Every advertised variant is actually shared. Cached previews stay private.
        for v in &mut variants {v.peers=0;v.selected=false;}
        let proofs = variants
            .iter()
            .map(|v| {
                let data = bytes(&state, &q.audio, &v.hash)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .ok_or(StatusCode::NOT_FOUND)?;
                Ok(content_hash(&q.nonce, "possession", &data))
            })
            .collect::<Result<Vec<_>, StatusCode>>()?;
        let body = serde_json::to_string(&Manifest {
            audio: q.audio,
            nonce: q.nonce,
            variants,
            proofs,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(Json(Envelope {
            signature: hex::encode(state.identity.sign(body.as_bytes())),
            peer: state.identity.public_hex(),
            body,
        }))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}
#[derive(Deserialize)]
pub struct DataQuery {
    audio: String,
    hash: String,
}
pub async fn serve_data(
    State(state): State<AppState>,
    Query(q): Query<DataQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], Vec<u8>), StatusCode> {
    if !hash_valid(&q.audio) || !hash_valid(&q.hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let data = tokio::task::spawn_blocking(move || {
        if !state.db.audio_verified(&q.audio)? {return Ok(None);}
        let offered = state.db.conn().prepare("SELECT 1 FROM metadata_variants WHERE audio=?1 AND hash=?2 AND (selected OR contributed)")?.exists(params![q.audio,q.hash])?;
        if offered { bytes(&state, &q.audio, &q.hash) } else { Ok(None) }
    })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], data))
}
fn verify(envelope: Envelope, audio: &str, nonce: &str) -> anyhow::Result<(String, Manifest)> {
    anyhow::ensure!(
        envelope.body.len() <= MAX_MANIFEST
            && hash_valid(&envelope.peer)
            && envelope.signature.len() == 128,
        "invalid manifest"
    );
    let key: [u8; 32] = hex::decode(&envelope.peer)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid peer"))?;
    VerifyingKey::from_bytes(&key)?.verify_strict(
        envelope.body.as_bytes(),
        &Signature::from_slice(&hex::decode(envelope.signature)?)?,
    )?;
    let manifest: Manifest = serde_json::from_str(&envelope.body)?;
    anyhow::ensure!(
        manifest.audio == audio
            && manifest.nonce == nonce
            && manifest.variants.len() <= MAX_VARIANTS,
        "wrong manifest"
    );
    anyhow::ensure!(
        manifest.proofs.len() == manifest.variants.len()
            && manifest.proofs.iter().all(|p| hash_valid(p)),
        "invalid possession proofs"
    );
    let mut hashes = HashSet::new();
    let total: u64 = manifest.variants.iter().map(|v| u64::from(v.size)).sum();
    anyhow::ensure!(total <= 4 * 1024 * 1024, "manifest data budget exceeded");
    for v in &manifest.variants {
        anyhow::ensure!(
            hash_valid(&v.hash)
                && matches!(v.kind.as_str(), "cover" | "lyrics")
                && v.size as usize <= MAX_DATA
                && hashes.insert(&v.hash),
            "invalid variant"
        );
    }
    Ok((envelope.peer, manifest))
}
async fn fetch_peer(
    state: &AppState,
    node: SocketAddr,
    audio: &str,
) -> anyhow::Result<(String, Vec<(MetadataVariant, Vec<u8>)>)> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)?;
    let nonce = hex::encode(random);
    let response = state
        .http
        .get(format!("http://{node}{PATH}?audio={audio}&nonce={nonce}"))
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    let envelope = crate::reach::read_json(response, MAX_MANIFEST * 2).await?;
    let (peer, manifest) = verify(envelope, audio, &nonce)?;
    anyhow::ensure!(peer != state.identity.public_hex(), "self response");
    let mut results = Vec::new();
    // Fresh nonce-bound proofs allow reuse of previously verified bytes.
    // The peer must know the bytes; a repeated manifest or public content hash is insufficient.
    for (v, proof) in manifest.variants.into_iter().zip(manifest.proofs) {
        let state2 = state.clone();
        let audio2 = audio.to_string();
        let hash = v.hash.clone();
        let cached = tokio::task::spawn_blocking(move || bytes(&state2, &audio2, &hash)).await??;
        if let Some(data) = cached {
            anyhow::ensure!(
                content_hash(&nonce, "possession", &data) == proof,
                "invalid possession proof"
            );
            results.push((v, data));
            continue;
        }
        let mut response = state
            .http
            .get(format!(
                "http://{node}{DATA_PATH}?audio={audio}&hash={}",
                v.hash
            ))
            .timeout(Duration::from_secs(3))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success()
                && response.content_length().unwrap_or(0) <= v.size as u64,
            "invalid content length"
        );
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                chunk.len() <= (v.size as usize).saturating_sub(data.len()),
                "attachment too large"
            );
            data.extend_from_slice(&chunk);
        }
        anyhow::ensure!(
            data.len() == v.size as usize && content_hash(audio, &v.kind, &data) == v.hash,
            "content hash mismatch"
        );
        anyhow::ensure!(
            content_hash(&nonce, "possession", &data) == proof,
            "invalid possession proof"
        );
        results.push((v, data));
    }
    Ok((peer, results))
}
/// Only the authenticated player can initiate network fetching. Public handlers
/// serve stored data without recursive requests. One bounded refresh at a time.
pub async fn list(
    state: &AppState,
    id: i64,
    refresh: bool,
) -> anyhow::Result<Vec<MetadataVariant>> {
    let state2 = state.clone();
    let audio = tokio::task::spawn_blocking(move || audio(&state2, id)).await??;
    if refresh {
        let _guard = state.metadata_work.lock().await;
        let cached = {
            let conn = state.db.conn();
            conn.query_row(
                "SELECT checked_at FROM metadata_checks WHERE audio=?1",
                [&audio],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|t| now().saturating_sub(t) < 15)
        };
        if cached {
            let state = state.clone();
            return tokio::task::spawn_blocking(move || list_local(&state, &audio)).await?;
        }
        {
            let conn = state.db.conn();
            conn.execute(
                "DELETE FROM metadata_checks WHERE checked_at<?1",
                [now().saturating_sub(60)],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO metadata_checks(audio,checked_at) VALUES(?1,?2)",
                params![audio, now()],
            )?;
        }
        let state2 = state.clone();
        let hash = audio.clone();
        let nodes:Vec<SocketAddr>=tokio::task::spawn_blocking(move|| {
            let conn=state2.db.conn();let mut q=conn.prepare("SELECT DISTINCT COALESCE(s.relay,s.node) FROM sources s JOIN entries e ON e.id=s.entry_id WHERE e.audio_sha256=?1 AND s.seen_at>=?2 AND COALESCE(s.relay,s.node) IS NOT NULL ORDER BY s.seen_at DESC LIMIT 8")?;
            let nodes=q.query_map(params![hash,now().saturating_sub(60)],|r|r.get::<_,String>(0))?.filter_map(|r|r.ok()?.parse().ok()).collect();
            anyhow::Ok(nodes)
        }).await??;
        let nodes = nodes
            .into_iter()
            .chain(state.config.peers.iter().copied())
            .chain(state.lan.addresses())
            .filter(|n| Some(*n) != state.p2p_addr())
            .collect::<HashSet<_>>();
        let work = async {
            for node in nodes.into_iter().take(8) {
                if let Ok(Ok((peer, variants))) =
                    tokio::time::timeout(Duration::from_secs(5), fetch_peer(state, node, &audio))
                        .await
                {
                    let state = state.clone();
                    let audio = audio.clone();
                    tokio::task::spawn_blocking(move || {
                        // Expire the previous vote even if this peer changed selection.
                        state.db.conn().execute("DELETE FROM metadata_sources WHERE audio=?1 AND peer=?2 OR seen_at<?3",params![audio,peer,now().saturating_sub(60)])?;
                        for (v,data) in variants {
                            if put(&state,&audio,&v.kind,&data,false,false).is_ok() {
                                let conn=state.db.conn();
                                conn.execute("INSERT OR REPLACE INTO metadata_sources(audio,hash,peer,seen_at) VALUES(?1,?2,?3,?4)",params![audio,v.hash,peer,now()])?;
                            }
                        }
                        anyhow::Ok(())
                    }).await??;
                }
            }
            anyhow::Ok(())
        };
        let _ = tokio::time::timeout(Duration::from_secs(20), work).await;
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || list_local(&state, &audio)).await?
}

/// A relay accepts attachments only during an authenticated, outstanding upload.
pub async fn accept_relay(state: &AppState, id: i64, bytes: Vec<u8>) -> anyhow::Result<()> {
    use base64::Engine;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Attachment {
        kind: String,
        data: String,
    }
    anyhow::ensure!(bytes.len() <= MAX_DATA * 2, "relay attachment too large");
    let attachment: Attachment = serde_json::from_slice(&bytes)?;
    let data = base64::engine::general_purpose::STANDARD.decode(attachment.data)?;
    publish(state, id, &attachment.kind, data, true).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifests_bind_audio_nonce_and_content_and_ignore_remote_ratings() {
        let dir = tempfile::tempdir().unwrap();
        let identity = crate::identity::Identity::load_or_create(&dir.path().join("key")).unwrap();
        let audio = "a".repeat(64);
        let nonce = "b".repeat(64);
        let data = b"[00:00.00]Words";
        let mut manifest = Manifest {
            audio: audio.clone(),
            nonce: nonce.clone(),
            variants: vec![MetadataVariant {
                hash: content_hash(&audio, "lyrics", data),
                kind: "lyrics".into(),
                size: data.len() as u32,
                selected: true,
                peers: u32::MAX,
            }],
            proofs: vec![content_hash(&nonce, "possession", data)],
        };
        let signed = |m: &Manifest| {
            let body = serde_json::to_string(m).unwrap();
            Envelope {
                signature: hex::encode(identity.sign(body.as_bytes())),
                peer: identity.public_hex(),
                body,
            }
        };
        assert!(verify(signed(&manifest), &audio, &nonce).is_ok());
        assert!(verify(signed(&manifest), &"c".repeat(64), &nonce).is_err());
        assert!(verify(signed(&manifest), &audio, &"d".repeat(64)).is_err());
        let mut forged = signed(&manifest);
        forged.body = forged.body.replace("lyrics", "covers");
        assert!(verify(forged, &audio, &nonce).is_err());
        manifest.variants[0].size = u32::MAX;
        assert!(verify(signed(&manifest), &audio, &nonce).is_err());
        manifest.variants[0].size = data.len() as u32;
        manifest.variants.push(manifest.variants[0].clone());
        manifest.proofs.push(manifest.proofs[0].clone());
        assert!(verify(signed(&manifest), &audio, &nonce).is_err());
        assert_ne!(
            content_hash(&nonce, "possession", data),
            content_hash(&"e".repeat(64), "possession", data)
        );
    }
    #[test]
    fn attachments_reject_control_characters_long_lines_and_non_images() {
        assert!(!valid("lyrics", b"text\0payload"));
        assert!(!valid("lyrics", &vec![b'x'; 16 * 1024 + 1]));
        assert!(!valid("lyrics", &vec![b'\n'; MAX_DATA]));
        assert!(!valid("cover", b"PK\x03\x04archive"));
        assert!(!valid("cover", &vec![0; MAX_DATA + 1]));
        assert!(valid("lyrics", b"[00:00.00]Text\nAnother line"));
        assert_ne!(
            content_hash(&"a".repeat(64), "lyrics", b"text"),
            content_hash(&"b".repeat(64), "lyrics", b"text")
        );
    }
}

#[cfg(test)]
mod lyrics_security_tests {
    use super::*;
    use crate::{
        auth::generate_token, config::Config, db::State as TrackState, ingest, library, AppState,
    };
    #[tokio::test]
    async fn poisoned_cached_lyrics_are_neither_selected_nor_advertised_or_served() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Config::isolated(dir.path().join("node")), &generate_token())
            .await
            .unwrap();
        let input =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
        let ingested = ingest::ingest(
            &input,
            "Artist - Song.flac",
            &state.storage.tmp_dir(),
            &state.config.limits,
        )
        .unwrap();
        let id = library::share(&state, ingested).await.unwrap().id;
        let audio = audio(&state, id).unwrap();
        let good = publish(&state, id, "lyrics", b"[00:00.00]Valid".to_vec(), true)
            .await
            .unwrap();
        let end = state.db.torrent(id).unwrap().unwrap().duration_ms + 1;
        let after_end = format!(
            "[{:02}:{:02}.{:03}]after end",
            end / 60_000,
            (end / 1000) % 60,
            end % 1000
        );
        for text in [
            "[-00:01]bad",
            "[00:-01]bad",
            "[00:60]bad",
            after_end.as_str(),
            "[offset:-100000]bad",
            "[00:00]bad\0text",
            "[18446744073709551615:00]bad",
        ] {
            assert!(
                publish(&state, id, "lyrics", text.as_bytes().to_vec(), true)
                    .await
                    .is_err(),
                "{text}"
            );
            assert!(crate::lyrics::publish(&state, id, text.into())
                .await
                .is_err());
            use base64::Engine;
            let relay=serde_json::to_vec(&serde_json::json!({"kind":"lyrics","data":base64::engine::general_purpose::STANDARD.encode(text)})).unwrap();
            assert!(accept_relay(&state, id, relay).await.is_err());
            // Simulate data saved by a previous version, with a correct content hash.
            let hash = content_hash(&audio, "lyrics", text.as_bytes());
            state.db.conn().execute("INSERT INTO metadata_variants(audio,hash,kind,data,selected,contributed) VALUES(?1,?2,'lyrics',?3,0,1)",params![audio,hash,text.as_bytes()]).unwrap();
            assert!(select(&state, id, hash.clone()).await.is_err());
            assert!(data(&state, id, hash.clone()).await.unwrap().is_none());
            let nonce = "b".repeat(64);
            let Json(envelope) = serve(
                State(state.clone()),
                Query(ManifestQuery {
                    audio: audio.clone(),
                    nonce: nonce.clone(),
                }),
            )
            .await
            .unwrap();
            let (_, manifest) = verify(envelope, &audio, &nonce).unwrap();
            assert!(!manifest.variants.iter().any(|v| v.hash == hash));
            assert_eq!(
                serve_data(
                    State(state.clone()),
                    Query(DataQuery {
                        audio: audio.clone(),
                        hash
                    })
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            selected(&state, id, "lyrics").await.unwrap().unwrap(),
            b"[00:00.00]Valid"
        );
        assert!(list(&state, id, false)
            .await
            .unwrap()
            .iter()
            .any(|v| v.hash == good));
        // Possessing cached bytes before audio validation must not make a relay.
        state.db.set_state(id, TrackState::Fetching).unwrap();
        let nonce = "c".repeat(64);
        let Json(envelope) = serve(
            State(state.clone()),
            Query(ManifestQuery {
                audio: audio.clone(),
                nonce: nonce.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(verify(envelope, &audio, &nonce)
            .unwrap()
            .1
            .variants
            .is_empty());
        assert_eq!(
            serve_data(State(state.clone()), Query(DataQuery { audio, hash: good }))
                .await
                .unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }
}

#[cfg(test)]
mod hostile_peer_test {
    use super::*;
    #[tokio::test]
    async fn custom_peer_cannot_publish_negative_lyrics_with_valid_hash_signature_and_proof() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut config = crate::config::Config::isolated(dir.path().join("receiver"));
        config.peers = vec![addr];
        let state = AppState::new(config, &crate::auth::generate_token())
            .await
            .unwrap();
        let file =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
        let ingested = crate::ingest::ingest(
            &file,
            "Artist - Track.flac",
            &state.storage.tmp_dir(),
            &state.config.limits,
        )
        .unwrap();
        let id = crate::library::share(&state, ingested).await.unwrap().id;
        let good = publish(&state, id, "lyrics", b"[00:00.00]Good".to_vec(), true)
            .await
            .unwrap();
        let key = std::sync::Arc::new(
            crate::identity::Identity::load_or_create(&dir.path().join("attacker.key")).unwrap(),
        );
        let bad = b"[00:00.00]Looks valid\n[-00:01]Negative";
        let router = axum::Router::new()
            .route(
                PATH,
                axum::routing::get(move |Query(q): Query<ManifestQuery>| {
                    let key = key.clone();
                    async move {
                        let manifest = Manifest {
                            audio: q.audio.clone(),
                            nonce: q.nonce.clone(),
                            variants: vec![MetadataVariant {
                                hash: content_hash(&q.audio, "lyrics", bad),
                                kind: "lyrics".into(),
                                size: bad.len() as u32,
                                selected: true,
                                peers: u32::MAX,
                            }],
                            proofs: vec![content_hash(&q.nonce, "possession", bad)],
                        };
                        let body = serde_json::to_string(&manifest).unwrap();
                        Json(Envelope {
                            signature: hex::encode(key.sign(body.as_bytes())),
                            peer: key.public_hex(),
                            body,
                        })
                    }
                }),
            )
            .route(DATA_PATH, axum::routing::get(|| async { bad.as_slice() }));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let found = list(&state, id, true).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hash, good);
        assert_eq!(
            found[0].peers, 1,
            "a fabricated count must not inflate valid variants"
        );
        let audio = audio(&state, id).unwrap();
        let poisoned = content_hash(&audio, "lyrics", bad);
        assert!(data(&state, id, poisoned).await.unwrap().is_none());
        task.abort();
    }
}
