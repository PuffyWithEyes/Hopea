// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tracks this node has: sharing new ones, downloading others' tracks,
//! checking downloads, and making room in the cache.

use std::{net::SocketAddr, time::Duration};

use hopea_common::api::Track;

use crate::{
    db::{now, State, TorrentRow},
    entry::{Entry, TrackMeta},
    ingest::{self, Ingested},
    p2p, relay,
    torrent_info::{self, InfoDict},
    torrents::Handle,
    AppState,
};

/// Result of sharing a validated upload.
pub struct Shared {
    pub id: i64,
    pub duplicate: bool,
}

/// Shares a validated upload: moves it into place, signs an entry for it,
/// and starts seeding. Blocking parts run off the executor.
pub async fn share(state: &AppState, ingested: Ingested) -> anyhow::Result<Shared> {
    let _guard = state.share_work.lock().await;
    let db = state.db.clone();
    let audio = ingested.audio_sha256.clone();
    if let Some(id) = tokio::task::spawn_blocking(move || db.own_track_with_audio(&audio)).await?? {
        if let Some(cover) = ingested.cover_jpeg {
            crate::metadata::publish(state, id, "cover", cover, false).await?;
        }
        return Ok(Shared {
            id,
            duplicate: true,
        });
    }

    let cover_variant = ingested.cover_jpeg.clone();
    let state2 = state.clone();
    let (id, info_hash, info_bytes) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let state = state2;
        let info = InfoDict::for_file(&ingested.file, ingested.format)?;
        let info_bytes = info.encode();
        let info_hash = torrent_info::info_hash(&info_bytes);
        let tags = &ingested.tags;
        let meta = TrackMeta {
            title: tags.title.clone().unwrap_or_else(|| "Untitled".into()),
            artist: tags.artist.clone(),
            album: tags.album.clone(),
            album_artist: tags.album_artist.clone(),
            track_number: tags.track_number,
            year: tags.year,
            genre: tags.genre.clone(),
            duration_ms: ingested.duration_ms,
            format: ingested.format,
            codec: ingested.codec.to_string(),
            sample_rate: ingested.sample_rate,
            channels: ingested.channels,
            size: ingested.size,
            sha256: ingested.sha256.clone(),
            audio_sha256: ingested.audio_sha256.clone(),
        };
        let entry = Entry::create(
            &state.identity,
            state.config.name.as_deref(),
            now(),
            info_bytes.clone(),
            meta,
            ingested.cover_jpeg.as_deref().map(sha256_hex),
        );

        if let Some(ref jpeg) = ingested.cover_jpeg {
            state.storage.store_cover(&info_hash, jpeg)?;
        }
        state
            .storage
            .store_track(ingested.file, &info_hash, ingested.format)?;
        let id = state.db.insert_entry(&entry, State::Own)?;
        state
            .db
            .set_peaks(id, &serde_json::to_string(&ingested.peaks)?)?;
        if ingested.cover_jpeg.is_some() {
            state.db.set_has_cover(id)?;
        }
        Ok((id, info_hash, info_bytes))
    })
    .await??;

    state
        .torrents
        .add(&info_hash, &info_bytes, Vec::new())
        .await?;
    if let Some(cover) = cover_variant {
        crate::metadata::publish(state, id, "cover", cover, true).await?;
    }
    state.tracks_changed();
    Ok(Shared {
        id,
        duplicate: false,
    })
}

/// Makes sure a track's torrent is in the session, starting a download if
/// the file isn't here yet. Returns `None` for tracks that failed
/// validation before.
pub async fn ensure_torrent(state: &AppState, row: &TorrentRow) -> anyhow::Result<Option<Handle>> {
    if row.state == State::Rejected {
        return Ok(None);
    }
    if let Some(handle) = state.torrents.get(&row.info_hash) {
        return Ok(Some(handle));
    }

    let db = state.db.clone();
    let id = row.id;
    // The nodes that returned the track in a search; the BitTorrent session
    // looks for more in the DHT on its own, without delaying the start
    let sources = tokio::task::spawn_blocking(move || db.sources(id)).await??;
    let mut peers: Vec<SocketAddr> = sources
        .iter()
        .filter(|s| s.relay.is_none())
        .map(|s| s.addr)
        .collect();
    if peers.is_empty() && !row.state.has_file() {
        // Only relays have it: one of them fetches it from its client first
        for source in sources.iter().filter(|s| s.relay.is_some()) {
            let relay = source.relay.unwrap();
            match relay::request_fetch(state, relay, &row.info_hash).await {
                Ok(()) => {
                    peers.push(source.addr);
                    break;
                }
                Err(err) => tracing::info!("Relay {relay} could not provide the track: {err:#}"),
            }
        }
    }
    // Search answers don't carry the torrent itself: get it from a node
    // that answered, now that it is needed
    let mut info = row.info.clone();
    if info.is_empty() {
        let nodes: Vec<SocketAddr> = sources.iter().filter_map(|s| s.relay.or(s.node)).collect();
        info = p2p::fetch_info(state, row.id, &nodes).await?;
    }
    let handle = state.torrents.add(&row.info_hash, &info, peers).await?;
    let mut row = row.clone();
    row.info = info;
    let row = &row;

    if row.state == State::Found || row.state == State::Fetching {
        if row.state == State::Found {
            let db = state.db.clone();
            tokio::task::spawn_blocking(move || db.set_state(id, State::Fetching)).await??;
        }
        tokio::spawn(watch_download(state.clone(), row.clone(), handle.clone()));
        let state = state.clone();
        let id = row.id;
        tokio::spawn(async move {
            let _ = crate::lyrics::fetch(&state, id).await;
        });
    }
    Ok(Some(handle))
}

/// Waits for a download to finish, then validates it: a valid file is kept
/// and shared on, anything else is deleted and never fetched again.
async fn watch_download(state: AppState, row: TorrentRow, handle: Handle) {
    if let Err(err) = handle.wait_until_completed().await {
        tracing::debug!("Download of {} stopped: {err:#}", row.info_hash);
        return;
    }

    let _permit = state.validations.clone().acquire_owned().await;
    let state2 = state.clone();
    let row2 = row.clone();
    let verdict = tokio::task::spawn_blocking(move || {
        let _permit = _permit;
        let state = state2;
        let row = row2;
        let path = state.storage.media_path(&row.info_hash, row.format);
        ingest::verify_download(
            &path,
            row.format,
            &row.sha256,
            &row.audio_sha256,
            row.duration_ms,
            &state.storage.tmp_dir(),
            &state.config.limits,
        )
    })
    .await;

    match verdict {
        Ok(Ok(ingested)) => {
            let db = state.db.clone();
            let peaks = serde_json::to_string(&ingested.peaks).unwrap_or_default();
            let id = row.id;
            let _ = tokio::task::spawn_blocking(move || {
                db.set_peaks(id, &peaks)?;
                db.set_verified_duration(id, ingested.duration_ms)?;
                db.set_state(id, State::Held)
            })
            .await;
            tracing::info!("Downloaded and verified {}", row.info_hash);
            state.tracks_changed();
            make_room(&state).await;
        }
        Ok(Err(rejection)) => {
            tracing::warn!(
                "Rejected download {}: {rejection} ({})",
                row.info_hash,
                rejection.code()
            );
            state.torrents.remove(&row.info_hash).await;
            state.storage.remove(&row.info_hash);
            let db = state.db.clone();
            let _ =
                tokio::task::spawn_blocking(move || db.set_state(row.id, State::Rejected)).await;
        }
        Err(err) => tracing::error!("Validation task failed: {err}"),
    }
}

/// Deletes the least recently played downloads until the cache fits.
/// Favorites and own tracks are never removed.
pub async fn make_room(state: &AppState) {
    let db = state.db.clone();
    let limit = state.config.cache_bytes;
    let evict = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(i64, String)>> {
        let mut used = db.bytes_in_state(State::Held)?;
        let mut out = Vec::new();
        for (id, info_hash, size) in db.eviction_candidates()? {
            if used <= limit {
                break;
            }
            used = used.saturating_sub(size);
            out.push((id, info_hash));
        }
        Ok(out)
    })
    .await;
    let Ok(Ok(evict)) = evict else {
        return;
    };
    for (id, info_hash) in evict {
        tracing::debug!("Evicting {info_hash} from the cache");
        state.torrents.remove(&info_hash).await;
        state.storage.remove(&info_hash);
        let db = state.db.clone();
        let _ = tokio::task::spawn_blocking(move || {
            db.clear_cached_cover(id)?;
            db.set_state(id, State::Found)
        })
        .await;
    }
}

/// Stops sharing an own track and forgets it.
pub async fn unshare(state: &AppState, id: i64, info_hash: &str) -> anyhow::Result<()> {
    state.torrents.remove(info_hash).await;
    state.storage.remove(info_hash);
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete(id)).await??;
    state.tracks_changed();
    Ok(())
}

/// Favorites control the personal playlist. Removing a star preserves local
/// audio and its sharing state; cache eviction is a separate operation.
pub async fn set_favorite(state: &AppState, id: i64, favorite: bool) -> anyhow::Result<bool> {
    let db = state.db.clone();
    let (found, row) = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        Ok((db.set_favorite(id, favorite)?, db.torrent(id)?))
    })
    .await??;
    match row {
        Some(row) if found && favorite => {
            ensure_torrent(state, &row).await?;
        }
        _ => {}
    }
    Ok(found)
}

/// Re-adds the torrents of every track this node has or was downloading,
/// and keeps the catalog tidy from then on.
pub async fn resume(state: AppState) {
    let db = state.db.clone();
    let rows = match tokio::task::spawn_blocking(move || db.torrents_to_resume()).await {
        Ok(Ok(rows)) => rows,
        _ => return,
    };
    for row in rows {
        match ensure_torrent(&state, &row).await {
            Ok(Some(handle)) if row.state == State::Held => {
                let db = state.db.clone();
                let id = row.id;
                if !tokio::task::spawn_blocking(move || db.duration_verified(id))
                    .await
                    .unwrap_or(Ok(false))
                    .unwrap_or(false)
                {
                    // One-time revalidation of cached files from before duration checks.
                    tokio::spawn(watch_download(state.clone(), row, handle));
                }
            }
            Err(err) => tracing::warn!("Unable to resume {}: {err:#}", row.info_hash),
            _ => {}
        }
    }
    announce_own_covers(&state).await;
    state.tracks_changed();

    loop {
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        let db = state.db.clone();
        let _ = tokio::task::spawn_blocking(move || db.prune_found()).await;
    }
}

/// The track as the player sees it.
pub async fn track(state: &AppState, id: i64) -> anyhow::Result<Option<Track>> {
    let db = state.db.clone();
    Ok(tokio::task::spawn_blocking(move || db.track(id))
        .await??
        .map(|row| row.track))
}

const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Pictures chosen for uploads still to come, at most
const MAX_PENDING_COVERS: usize = 16;

/// Decodes and re-encodes a picture chosen for uploads that follow, and
/// keeps it for them. Returns the name they give it: its SHA-256.
pub async fn add_pending_cover(state: &AppState, image: Vec<u8>) -> Option<String> {
    let permit = state.validations.clone().try_acquire_owned().ok()?;
    let jpeg = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        ingest::reencode_cover(&image)
    })
    .await
    .ok()??;
    let sha = sha256_hex(&jpeg);
    let mut pending = state.pending_covers.lock().unwrap();
    pending.retain(|(s, _)| *s != sha);
    if pending.len() >= MAX_PENDING_COVERS {
        pending.pop_front();
    }
    pending.push_back((sha.clone(), jpeg));
    Some(sha)
}

/// A picture kept by [`add_pending_cover`].
pub fn pending_cover(state: &AppState, sha: &str) -> Option<Vec<u8>> {
    let pending = state.pending_covers.lock().unwrap();
    pending
        .iter()
        .find(|(s, _)| s == sha)
        .map(|(_, jpeg)| jpeg.clone())
}

/// Signs the entry of one of this node's own tracks again, naming the cover
/// art selected by its owner. Other nodes accept newer signed artwork
/// only from the original publisher.
pub async fn announce_cover(state: &AppState, id: i64, image: Vec<u8>) -> anyhow::Result<()> {
    // Preserve the previous choice before publishing another alternative.
    if let Some(previous) = cover_for_player(state, id).await? {
        crate::metadata::publish(state, id, "cover", previous, false).await?;
    }
    crate::metadata::publish(state, id, "cover", image.clone(), true).await?;
    let db = state.db.clone();
    if !tokio::task::spawn_blocking(move || db.cover_state(id))
        .await??
        .is_some_and(|c| c.own)
    {
        return Ok(());
    }
    let state2 = state.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let state = state2;
        anyhow::ensure!(
            state.db.cover_state(id)?.is_some_and(|cover| cover.own),
            "not an own track"
        );
        let jpeg = ingest::reencode_cover(&image)
            .ok_or_else(|| anyhow::anyhow!("only PNG and JPEG pictures can be used"))?;
        let (body, sig) = state
            .db
            .entry_bytes(id)?
            .ok_or_else(|| anyhow::anyhow!("unknown track"))?;
        let info = state
            .db
            .torrent(id)?
            .map(|t| t.info)
            .filter(|i| !i.is_empty());
        let old = Entry::from_stored(body, sig, info, &state.config.limits, now())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if old.cover_sha256.as_deref() == Some(sha256_hex(&jpeg).as_str()) {
            return Ok(());
        }
        let info_bytes = old
            .info_bytes
            .clone()
            .ok_or_else(|| anyhow::anyhow!("own track without a torrent"))?;
        state.storage.store_cover(&old.info_hash, &jpeg)?;
        // Later than the old entry, so that every node takes the new one
        let created_at = now().max(old.created_at + 1);
        let entry = Entry::create(
            &state.identity,
            state.config.name.as_deref(),
            created_at,
            info_bytes,
            old.track,
            Some(sha256_hex(&jpeg)),
        );
        state.db.replace_own_entry(id, &entry)?;
        state.db.set_has_cover(id)?;
        Ok(())
    })
    .await??;
    state.tracks_changed();
    Ok(())
}

/// A track's cover art for the player, fetching it from a node that has it
/// if needed. What comes from another node must match the hash in the
/// signed entry, and is decoded and re-encoded before the player gets it.
pub async fn cover_for_player(state: &AppState, id: i64) -> anyhow::Result<Option<Vec<u8>>> {
    if let Some(data) = crate::metadata::selected(state, id, "cover").await? {
        return Ok(Some(data));
    }
    original_cover_for_player(state, id).await
}
pub async fn original_cover_for_player(
    state: &AppState,
    id: i64,
) -> anyhow::Result<Option<Vec<u8>>> {
    let db = state.db.clone();
    let Some(cover) = tokio::task::spawn_blocking(move || db.cover_state(id)).await?? else {
        return Ok(None);
    };
    let path = state.storage.cover_path(&cover.info_hash);
    if cover.here {
        match tokio::fs::read(&path).await {
            Ok(data) => {
                if cover.own {
                    return Ok(Some(data));
                }
                return Ok(
                    tokio::task::spawn_blocking(move || ingest::reencode_cover(&data)).await?,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let db = state.db.clone();
                tokio::task::spawn_blocking(move || db.clear_cached_cover(id)).await??;
                // A removed cache file is a cache miss, not an internal error.
                // Keep the signed hash and recover a verified copy from peers.
            }
            Err(error) => return Err(error.into()),
        }
    }
    let Some(expected) = cover.sha256 else {
        return Ok(None);
    };

    let db = state.db.clone();
    let sources = tokio::task::spawn_blocking(move || db.sources(id)).await??;
    let nodes: Vec<std::net::SocketAddr> =
        sources.iter().filter_map(|s| s.relay.or(s.node)).collect();
    for node in nodes {
        let url = format!(
            "http://{node}{}?info_hash={}",
            p2p::COVER_PATH,
            cover.info_hash
        );
        let fetched = async {
            let mut response = state
                .http
                .get(url)
                .timeout(Duration::from_secs(10))
                .send()
                .await?
                .error_for_status()?;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                anyhow::ensure!(
                    chunk.len() <= MAX_COVER_BYTES.saturating_sub(body.len()),
                    "cover too large"
                );
                body.extend_from_slice(&chunk);
            }
            anyhow::Ok(body)
        };
        let data = match fetched.await {
            Ok(data) if sha256_hex(&data) == expected => data,
            Ok(_) => {
                tracing::info!("{node} sent cover art that doesn't match its track");
                continue;
            }
            Err(err) => {
                tracing::debug!("No cover from {node}: {err:#}");
                continue;
            }
        };
        // Kept as received, to pass on unchanged; the player gets a copy
        // made by this node's encoder
        state.storage.store_cover(&cover.info_hash, &data)?;
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || db.set_has_cover(id)).await??;
        return Ok(tokio::task::spawn_blocking(move || ingest::reencode_cover(&data)).await?);
    }
    Ok(None)
}

/// Signs the entries of own tracks that have cover art again, if they were
/// shared before covers were passed on, so that others get the covers.
pub async fn announce_own_covers(state: &AppState) {
    let db = state.db.clone();
    let Ok(Ok(ids)) = tokio::task::spawn_blocking(move || db.own_covers_unannounced()).await else {
        return;
    };
    for id in ids {
        let db = state.db.clone();
        let info_hash = tokio::task::spawn_blocking(move || db.cover_state(id))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .map(|c| c.info_hash);
        let Some(info_hash) = info_hash else { continue };
        match tokio::fs::read(state.storage.cover_path(&info_hash)).await {
            Ok(jpeg) => {
                if let Err(err) = announce_cover(state, id, jpeg).await {
                    tracing::warn!("Unable to announce the cover of {info_hash}: {err:#}");
                }
            }
            Err(err) => tracing::debug!("No cover file for {info_hash}: {err}"),
        }
    }
}
