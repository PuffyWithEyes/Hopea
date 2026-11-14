// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The API between the player and its node. It listens on loopback only.

use axum::{
    body::Body,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::StreamExt;
use hopea_common::{
    api::{
        self, ApiError, Me, PendingCover, SearchOrder, SpeedLimits, Track, TrackPage, Transfer,
        UploadResponse, COVER_HEADER, FALLBACK_COVER_HEADER, FILENAME_HEADER,
    },
    AudioFormat,
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::{services::ServeFile, set_header::SetResponseHeaderLayer, trace::TraceLayer};

use crate::{
    auth::Authorized,
    db::{State as TrackState, TrackRow},
    identity::fingerprint,
    ingest::{self, Rejection},
    keywords, library, p2p, recommend,
    stream::{self, PcmSource, TorrentFile},
    AppState,
};

const MAX_FILE_NAME_BYTES: usize = 255;
const MAX_PAGE_SIZE: u32 = 200;
const MAX_OFFSET: u32 = 1_000_000;
const FEED_SIZE: usize = 100;
/// Known tracks looked at to put an album together
const MAX_ALBUM_CANDIDATES: u32 = 500;
/// Largest picture accepted as cover art
const MAX_COVER_UPLOAD: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    let tracks = api::tracks_path();
    Router::new()
        .route(&api::me_path(), get(me))
        .route(
            &tracks,
            get(list_tracks)
                .post(upload)
                // Upload size is enforced while streaming the body to disk
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            &format!("{tracks}/{{id}}"),
            get(get_track).delete(delete_track),
        )
        .route(&format!("{tracks}/available"), get(available_tracks))
        .route(&format!("{tracks}/{{id}}/audio"), get(audio))
        .route(
            &format!("{tracks}/{{id}}/cover"),
            get(cover).put(update_cover),
        )
        .route(
            &api::covers_path(),
            post(add_cover).layer(DefaultBodyLimit::max(MAX_COVER_UPLOAD)),
        )
        .route(&format!("{tracks}/{{id}}/peaks"), get(peaks))
        .route(&format!("{tracks}/{{id}}/variants"), get(metadata_variants))
        .route(
            &format!("{tracks}/{{id}}/variants/{{hash}}"),
            get(metadata_data).put(metadata_select),
        )
        .route(
            &format!("{tracks}/{{id}}/lyrics"),
            get(get_lyrics)
                .put(set_lyrics)
                .layer(DefaultBodyLimit::max(crate::lyrics::MAX_TEXT)),
        )
        .route(&format!("{tracks}/{{id}}/transfer"), get(transfer))
        .route(&format!("{tracks}/{{id}}/prefetch"), post(prefetch))
        .route(&format!("{tracks}/{{id}}/file"), get(file))
        .route(&api::album_path(), get(album))
        .route(&api::limits_path(), get(limits).put(set_limits))
        .route(
            &format!("{tracks}/{{id}}/favorite"),
            put(add_favorite).delete(remove_favorite),
        )
        .route(&api::favorites_path(), get(favorites))
        .route(&api::recommendations_path(), get(recommendations))
        .fallback(|| async { ApiFailure::not_found() })
        // Keep browsers from ever treating a response as something other than
        // what it is, should anyone open a URL directly
        .layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'; sandbox",
        ))
        .layer(set_header(header::REFERRER_POLICY, "no-referrer"))
        .layer(set_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn set_header(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

/// An error response with a JSON [`ApiError`] body.
#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    error: ApiError,
}

impl ApiFailure {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            error: ApiError {
                error: code.into(),
                message: message.into(),
            },
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid access token is required",
        )
    }

    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Not found")
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!("Internal error: {err}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal error",
        )
    }

    fn fetching(err: anyhow::Error) -> Self {
        if err.is::<p2p::TrackUnavailable>() {
            Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "track_unavailable",
                "No available source could provide this track",
            )
        } else {
            Self::internal(err)
        }
    }

    fn rejected(rejection: Rejection) -> Self {
        let status = match rejection {
            Rejection::Internal(ref err) => return Self::internal(err),
            Rejection::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Rejection::MissingExtension | Rejection::UnsupportedExtension(_) => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        Self::new(status, rejection.code(), rejection.to_string())
    }

    fn unavailable(message: &str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(self.error)).into_response()
    }
}

type ApiResult<T> = Result<T, ApiFailure>;

/// Runs a blocking database call off the async executor.
async fn blocking<T, F>(state: &AppState, f: F) -> ApiResult<T>
where
    F: FnOnce(&crate::db::Db) -> rusqlite::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || f(&db))
        .await
        .map_err(ApiFailure::internal)?
        .map_err(ApiFailure::internal)
}

async fn me(State(state): State<AppState>, _: Authorized) -> ApiResult<Json<Me>> {
    let used_bytes = blocking(&state, |db| db.bytes_in_state(TrackState::Own)).await?;
    let key = state.identity.public_hex();
    Ok(Json(Me {
        name: crate::db::uploader_label(&key, state.config.name.as_deref()),
        used_bytes,
        quota_bytes: state.config.quota_bytes,
        max_upload_bytes: state.config.limits.max_upload_bytes,
        reachability: state.reach.get(),
    }))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    q: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    /// Also search other nodes (first page only)
    #[serde(default)]
    network: bool,
    #[serde(default)]
    discovery: bool,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    order: SearchOrder,
}

#[derive(Deserialize)]
struct AvailabilityQuery {
    ids: String,
    q: Option<String>,
    #[serde(default)]
    playback: bool,
}

async fn available_tracks(
    State(state): State<AppState>,
    _: Authorized,
    Query(query): Query<AvailabilityQuery>,
) -> ApiResult<Json<Vec<i64>>> {
    if query.ids.len() > 5000 {
        return Err(ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "invalid_ids",
            "Too many track IDs",
        ));
    }
    let ids: Vec<i64> = query
        .ids
        .split(',')
        .take(201)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| {
            ApiFailure::new(StatusCode::BAD_REQUEST, "invalid_ids", "Invalid track IDs")
        })?;
    if ids.len() > 200 {
        return Err(ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "invalid_ids",
            "Too many track IDs",
        ));
    }
    if query.playback {
        recommend::refresh_sources(&state).await;
        return Ok(Json(
            blocking(&state, move |db| db.available_playback_ids(&ids)).await?,
        ));
    }
    if let Some(q) = query.q.filter(|q| !q.trim().is_empty()) {
        p2p::refresh_sources(&state, &q.chars().take(200).collect::<String>()).await;
    }
    Ok(Json(
        blocking(&state, move |db| db.available_discovery_ids(&ids)).await?,
    ))
}

async fn list_tracks(
    State(state): State<AppState>,
    _: Authorized,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<TrackPage>> {
    let q = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0).min(MAX_OFFSET);
    let report = if query.network && offset == 0 && !q.trim().is_empty() {
        Some(p2p::search(&state, &q).await)
    } else {
        None
    };
    let order = query.order;
    let (tracks, total) = blocking(&state, move |db| {
        if query.local {
            db.local_tracks(limit, offset)
        } else if query.discovery {
            db.discover(&q, limit, offset, order)
        } else {
            db.search(&q, limit, offset, order)
        }
    })
    .await?;
    Ok(Json(TrackPage {
        tracks,
        total,
        nodes_answered: report.as_ref().map_or(0, |r| r.nodes_answered),
        report,
    }))
}

async fn track_row(state: &AppState, id: i64) -> ApiResult<TrackRow> {
    blocking(state, move |db| db.track(id))
        .await?
        .ok_or_else(ApiFailure::not_found)
}

async fn get_track(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Json<Track>> {
    Ok(Json(track_row(&state, id).await?.track))
}

/// Stops sharing one of this node's own tracks.
async fn delete_track(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let row = track_row(&state, id).await?;
    if !row.track.mine {
        return Err(ApiFailure::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only tracks shared from this computer can be removed",
        ));
    }
    library::unshare(&state, id, &row.info_hash)
        .await
        .map_err(ApiFailure::internal)?;
    tracing::info!(id, "Stopped sharing track");
    Ok(StatusCode::NO_CONTENT)
}

/// Serves a track for playback, with support for `Range` requests so that
/// the player can start and seek before the whole track is here.
///
/// Own tracks were validated when they were shared and are served as they
/// are. Everyone else's are decoded here and served as PCM (see
/// [`crate::stream`]), downloading them as they play.
async fn audio(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
    request: Request,
) -> ApiResult<Response> {
    let row = track_row(&state, id).await?;
    let _ = blocking(&state, move |db| db.mark_used(id)).await;
    if row.state == TrackState::Own {
        return own_audio(&state, &row, request).await;
    }

    let torrent = blocking(&state, move |db| db.torrent(id))
        .await?
        .ok_or_else(ApiFailure::not_found)?;
    if torrent.state == TrackState::Rejected {
        return Err(ApiFailure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "rejected",
            "This track was downloaded before and failed the safety checks",
        ));
    }

    // A file that is complete and verified is read from disk; anything else
    // through the torrent, which fetches the pieces being read first
    let source: Box<dyn symphonia::core::io::MediaSource> = if torrent.state == TrackState::Held {
        let path = state.storage.media_path(&torrent.info_hash, torrent.format);
        Box::new(std::fs::File::open(path).map_err(ApiFailure::internal)?)
    } else {
        let handle = library::ensure_torrent(&state, &torrent)
            .await
            .map_err(ApiFailure::fetching)?
            .ok_or_else(ApiFailure::not_found)?;
        let wait =
            tokio::time::timeout(state.config.stream_timeout, handle.wait_until_initialized());
        if !matches!(wait.await, Ok(Ok(()))) {
            return Err(ApiFailure::unavailable(
                "Nobody who has this track is online right now",
            ));
        }
        let file_stream = handle.stream(0).await.map_err(ApiFailure::internal)?;
        let len = file_stream.len();
        Box::new(TorrentFile::new(
            file_stream,
            len,
            tokio::runtime::Handle::current(),
            state.config.stream_timeout,
        ))
    };

    let range_header = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let limits = state.config.limits.clone();
    let (format, duration_ms) = (torrent.format, torrent.duration_ms);
    let pcm =
        tokio::task::spawn_blocking(move || PcmSource::open(source, format, duration_ms, &limits))
            .await
            .map_err(ApiFailure::internal)?
            .map_err(|err| {
                tracing::info!("Unable to play track {id}: {err:#}");
                ApiFailure::unavailable("The track could not be played")
            })?;

    let total = pcm.total_len();
    let (status, start, end) = match stream::parse_range(range_header.as_deref(), total) {
        Ok(None) => (StatusCode::OK, 0, total),
        Ok(Some((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end),
        Err(()) => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response())
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    tokio::task::spawn_blocking(move || {
        pcm.produce(start, end, |chunk| tx.blocking_send(chunk).is_ok());
    });
    let body = Body::from_stream(tokio_stream_from(rx).map(Ok::<_, std::convert::Infallible>));

    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    if status == StatusCode::PARTIAL_CONTENT {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{}/{total}", end - 1))
                .map_err(ApiFailure::internal)?,
        );
    }
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    Ok(response)
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> impl futures_util::Stream<Item = Vec<u8>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

async fn own_audio(state: &AppState, row: &TrackRow, request: Request) -> ApiResult<Response> {
    let track = &row.track;
    let path = state.storage.media_path(&row.info_hash, track.format);
    let response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(ApiFailure::internal)?;
    let (mut parts, body) = response.into_parts();
    if parts.status.is_success() {
        parts.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(track.format.mime_type()),
        );
        parts.headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}

async fn cover(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    track_row(&state, id).await?;
    let data = library::cover_for_player(&state, id)
        .await
        .map_err(ApiFailure::internal)?
        .ok_or_else(ApiFailure::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        data,
    )
        .into_response())
}

async fn update_cover(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
    Json(picture): Json<PendingCover>,
) -> ApiResult<Json<Track>> {
    track_row(&state, id).await?;
    let image = library::pending_cover(&state, &picture.cover).ok_or_else(|| {
        ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "cover_gone",
            "Upload the picture first",
        )
    })?;
    library::announce_cover(&state, id, image)
        .await
        .map_err(ApiFailure::internal)?;
    Ok(Json(track_row(&state, id).await?.track))
}

/// Takes a picture for uploads that follow: it is decoded and re-encoded
/// right away, and named in their [`COVER_HEADER`] or an own-track cover update.
async fn add_cover(
    State(state): State<AppState>,
    _: Authorized,
    body: Bytes,
) -> ApiResult<Json<PendingCover>> {
    let cover = library::add_pending_cover(&state, body.to_vec())
        .await
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "bad_image",
                "Only PNG and JPEG pictures can be used",
            )
        })?;
    Ok(Json(PendingCover { cover }))
}

/// The picture an upload header names, if it has the header.
fn named_cover(state: &AppState, headers: &HeaderMap, name: &str) -> ApiResult<Option<Vec<u8>>> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|sha| library::pending_cover(state, sha))
        .map(Some)
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "cover_gone",
                "The picture for this song is no longer here: choose it again",
            )
        })
}

/// Starts downloading a track that is about to be played. It is kept like
/// any played track: in the cache, shared on, and dropped when room is
/// needed unless it becomes a favorite.
async fn prefetch(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    // On-demand playback still downloads normally; only speculative work stops.
    if state.torrents.gaming() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let torrent = blocking(&state, move |db| db.torrent(id))
        .await?
        .ok_or_else(ApiFailure::not_found)?;
    library::ensure_torrent(&state, &torrent)
        .await
        .map_err(ApiFailure::fetching)?;
    Ok(StatusCode::ACCEPTED)
}

/// How a track's file is coming along, and how it is shared from here.
async fn transfer(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Json<Transfer>> {
    let row = track_row(&state, id).await?;
    let mut transfer = Transfer {
        progress: if row.track.local { 1.0 } else { 0.0 },
        ..Transfer::default()
    };
    if let Some(handle) = state.torrents.get(&row.info_hash) {
        let stats = handle.stats();
        if stats.total_bytes > 0 {
            transfer.progress = (stats.progress_bytes as f64 / stats.total_bytes as f64) as f32;
        }
        transfer.uploaded_bytes = stats.uploaded_bytes;
        if let Some(live) = stats.live {
            // librqbit counts speeds in MiB/s
            let bps = |mib: f64| (mib.max(0.0) * 1024.0 * 1024.0) as u64;
            transfer.peers = live.snapshot.peer_stats.live;
            transfer.download_bps = bps(live.download_speed.mbps);
            transfer.upload_bps = bps(live.upload_speed.mbps);
        }
    }
    Ok(Json(transfer))
}

async fn peaks(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let peaks = blocking(&state, move |db| db.peaks(id))
        .await?
        .ok_or_else(ApiFailure::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        peaks,
    )
        .into_response())
}

async fn get_lyrics(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Json<Option<String>>> {
    track_row(&state, id).await?;
    if let Some(text) = crate::metadata::selected(&state, id, "lyrics")
        .await
        .map_err(ApiFailure::internal)?
    {
        return Ok(Json(Some(
            String::from_utf8(text).map_err(ApiFailure::internal)?,
        )));
    }
    Ok(Json(
        crate::lyrics::fetch(&state, id)
            .await
            .map_err(ApiFailure::internal)?,
    ))
}

async fn set_lyrics(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
    text: String,
) -> ApiResult<StatusCode> {
    let row = track_row(&state, id).await?;
    crate::metadata::publish(&state, id, "lyrics", text.as_bytes().to_vec(), true)
        .await
        .map_err(|error| {
            ApiFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_lyrics",
                format!("Invalid lyrics: {error}"),
            )
        })?;
    if !row.track.mine {
        return Ok(StatusCode::NO_CONTENT);
    }
    crate::lyrics::publish(&state, id, text)
        .await
        .map_err(|_| {
            ApiFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_lyrics",
                "Lyrics could not be saved: check their size and contents",
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_favorite(state: &AppState, id: i64, favorite: bool) -> ApiResult<Json<Track>> {
    if !library::set_favorite(state, id, favorite)
        .await
        .map_err(ApiFailure::internal)?
    {
        return Err(ApiFailure::not_found());
    }
    Ok(Json(track_row(state, id).await?.track))
}

async fn add_favorite(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Json<Track>> {
    set_favorite(&state, id, true).await
}

async fn remove_favorite(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
) -> ApiResult<Json<Track>> {
    set_favorite(&state, id, false).await
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn favorites(
    State(state): State<AppState>,
    _: Authorized,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<TrackPage>> {
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0).min(MAX_OFFSET);
    let (tracks, total) = blocking(&state, move |db| db.favorites(limit, offset)).await?;
    Ok(Json(TrackPage {
        tracks,
        total,
        nodes_answered: 0,
        report: None,
    }))
}

#[derive(Debug, Deserialize)]
struct FeedQuery {
    #[serde(default)]
    refresh: bool,
}

async fn recommendations(
    State(state): State<AppState>,
    _: Authorized,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<TrackPage>> {
    let (tracks, nodes_answered) = recommend::feed(&state, query.refresh, FEED_SIZE).await;
    Ok(Json(TrackPage {
        total: tracks.len() as u64,
        tracks,
        nodes_answered,
        report: None,
    }))
}

/// Sends a track's file, for the player to save it. Only files this node
/// has in full and verified (its own, or downloads that passed validation)
/// are ever sent.
async fn file(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
    request: Request,
) -> ApiResult<Response> {
    let row = track_row(&state, id).await?;
    if !row.state.has_file() {
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            "not_here",
            "The song isn't on this computer yet",
        ));
    }
    own_audio(&state, &row, request).await
}

/// The speed limits in effect.
async fn limits(State(state): State<AppState>, _: Authorized) -> Json<SpeedLimits> {
    Json(state.torrents.limits())
}

/// Changes the speed limits right away. They are made fair first (see
/// [`SpeedLimits::fair`]), and the limits now in effect are returned.
async fn set_limits(
    State(state): State<AppState>,
    _: Authorized,
    Json(limits): Json<SpeedLimits>,
) -> Json<SpeedLimits> {
    let applied = state.torrents.set_limits(limits);
    tracing::info!(?applied, "Speed limits changed");
    Json(applied)
}

/// Normalized text for comparing album titles and artists.
fn album_key(text: &str) -> String {
    keywords::words(text).collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Deserialize)]
struct AlbumQuery {
    album: String,
    /// The album artist, or the artist of its tracks
    artist: Option<String>,
    /// Search other nodes for the album first
    #[serde(default)]
    network: bool,
}

/// The tracks of an album this node knows of, after searching the network
/// for it if asked, in track order. A song shared by several people is
/// listed once, preferring a copy that is already here.
async fn album(
    State(state): State<AppState>,
    _: Authorized,
    Query(query): Query<AlbumQuery>,
) -> ApiResult<Json<TrackPage>> {
    let album = album_key(&query.album);
    if album.is_empty() {
        return Err(ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "bad_album",
            "An album title is needed",
        ));
    }
    let artist = query
        .artist
        .as_deref()
        .map(album_key)
        .filter(|a| !a.is_empty());

    let nodes_answered = if query.network {
        let words = format!("{} {}", query.artist.as_deref().unwrap_or(""), query.album);
        p2p::search(&state, &words).await.nodes_answered
    } else {
        0
    };

    let q = query.album.clone();
    let (candidates, _) = blocking(&state, move |db| {
        db.search(&q, MAX_ALBUM_CANDIDATES, 0, SearchOrder::BestMatch)
    })
    .await?;
    let mut tracks: Vec<Track> = candidates
        .into_iter()
        .filter(|t| t.album.as_deref().map(album_key).as_deref() == Some(album.as_str()))
        .filter(|t| {
            artist.as_ref().is_none_or(|a| {
                [t.album_artist.as_deref(), t.artist.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|x| album_key(x) == *a)
            })
        })
        .collect();

    tracks.sort_by_key(|t| (!t.local, !t.mine));
    let mut seen = std::collections::HashSet::new();
    tracks.retain(|t| seen.insert((t.track_number, album_key(&t.title))));
    tracks.sort_by(|a, b| {
        (a.track_number.unwrap_or(u32::MAX), &a.title)
            .cmp(&(b.track_number.unwrap_or(u32::MAX), &b.title))
    });
    Ok(Json(TrackPage {
        total: tracks.len() as u64,
        tracks,
        nodes_answered,
        report: None,
    }))
}

/// Decodes and checks the upload's original file name. Only the extension
/// matters for validation; path components are discarded.
fn upload_file_name(headers: &HeaderMap) -> ApiResult<String> {
    let bad = || {
        ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "bad_file_name",
            format!("The {FILENAME_HEADER} header must contain the file name"),
        )
    };
    let raw = headers
        .get(FILENAME_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(bad)?;
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| bad())?;
    let name = decoded.rsplit(['/', '\\']).next().unwrap_or_default();
    if name.is_empty() || name.len() > MAX_FILE_NAME_BYTES || name.contains('\0') {
        return Err(bad());
    }
    Ok(name.to_string())
}

/// Shares a file: receives it as a raw request body (no multipart parsing),
/// with the original file name in [`FILENAME_HEADER`], validates it, and
/// starts sharing the sanitized result.
async fn upload(
    State(state): State<AppState>,
    _: Authorized,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<UploadResponse>)> {
    let config = state.config.clone();
    let limits = config.limits.clone();

    // Everything that can be checked before receiving the body is
    let file_name = upload_file_name(&headers)?;
    let cover = named_cover(&state, &headers, COVER_HEADER)?;
    let fallback_cover = named_cover(&state, &headers, FALLBACK_COVER_HEADER)?;
    if AudioFormat::from_file_name(&file_name).is_none() {
        let ext = file_name
            .rsplit_once('.')
            .map_or("", |(_, e)| e)
            .to_string();
        let rejection = if ext.is_empty() {
            Rejection::MissingExtension
        } else {
            Rejection::UnsupportedExtension(
                ext.chars()
                    .take(16)
                    .filter(|c| c.is_alphanumeric())
                    .collect(),
            )
        };
        return Err(ApiFailure::rejected(rejection));
    }

    let declared_len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let too_large =
        || ApiFailure::rejected(Rejection::TooLarge(limits.max_upload_bytes / (1024 * 1024)));
    if declared_len.is_some_and(|n| n > limits.max_upload_bytes) {
        return Err(too_large());
    }

    // Reserve both receive slots before reading the quota. Own uploads must
    // not race each other using the same remaining disk allowance.
    let _slot = state
        .uploads
        .clone()
        .try_acquire_many_owned(2)
        .map_err(|_| {
            ApiFailure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_uploads",
                "Wait for the other files to finish",
            )
        })?;
    let used = blocking(&state, |db| db.bytes_in_state(TrackState::Own)).await?;
    let quota_exceeded = || {
        ApiFailure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "quota_exceeded",
            "There is no more space set aside for shared music",
        )
    };
    if used > config.quota_bytes
        || declared_len.unwrap_or(0) > config.quota_bytes.saturating_sub(used)
    {
        return Err(quota_exceeded());
    }

    // Stream the body to a private temporary file, enforcing the size limit
    // and an idle timeout as it arrives
    let tmp = tempfile::Builder::new()
        .prefix("upload-")
        .tempfile_in(state.storage.tmp_dir())
        .map_err(ApiFailure::internal)?;
    let (std_file, tmp_path) = tmp.into_parts();
    let mut file = tokio::fs::File::from_std(std_file);
    let mut stream = body.into_data_stream();
    let mut received = 0u64;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10 * 60);
    loop {
        let idle_deadline = tokio::time::Instant::now() + config.upload_idle_timeout;
        let chunk = match tokio::time::timeout_at(deadline.min(idle_deadline), stream.next()).await
        {
            Err(_) => {
                return Err(ApiFailure::new(
                    StatusCode::REQUEST_TIMEOUT,
                    "timeout",
                    "The upload stalled",
                ))
            }
            Ok(None) => break,
            Ok(Some(Err(err))) => {
                tracing::debug!("Upload aborted: {err}");
                return Err(ApiFailure::new(
                    StatusCode::BAD_REQUEST,
                    "upload_aborted",
                    "The upload was interrupted",
                ));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        if chunk.len() as u64 > limits.max_upload_bytes.saturating_sub(received) {
            return Err(too_large());
        }
        received += chunk.len() as u64;
        if declared_len.is_some_and(|length| received > length) {
            return Err(ApiFailure::new(
                StatusCode::BAD_REQUEST,
                "size_mismatch",
                "The upload size does not match its declared length",
            ));
        }
        if received > config.quota_bytes.saturating_sub(used) {
            return Err(quota_exceeded());
        }
        file.write_all(&chunk).await.map_err(ApiFailure::internal)?;
    }
    file.flush().await.map_err(ApiFailure::internal)?;
    drop(file);

    if declared_len.is_some_and(|length| received != length) {
        return Err(ApiFailure::new(
            StatusCode::BAD_REQUEST,
            "size_mismatch",
            "The upload size does not match its declared length",
        ));
    }

    // Validation is CPU bound: bound the number running at once and keep it
    // off the async executor
    let _permit = state
        .validations
        .clone()
        .acquire_owned()
        .await
        .map_err(ApiFailure::internal)?;
    let work_dir = state.storage.tmp_dir();
    let name = file_name.clone();
    let ingested = tokio::task::spawn_blocking(move || {
        // Blocking work survives cancellation of its HTTP handler. Keep the
        // permit with the work so disconnects cannot bypass the CPU limit.
        let _permit = _permit;
        let result = ingest::ingest(&tmp_path, &name, &work_dir, &limits);
        drop(tmp_path);
        result
    })
    .await
    .map_err(ApiFailure::internal)?;

    let mut ingested = match ingested {
        Ok(ingested) => ingested,
        Err(rejection) => {
            tracing::info!(
                file = file_name,
                code = rejection.code(),
                "Rejected upload: {rejection}"
            );
            return Err(ApiFailure::rejected(rejection));
        }
    };

    // Others find songs by their names: a song nobody could find isn't
    // shared at all
    let title = ingested.tags.title.clone().unwrap_or_default();
    let audio = ingested.audio_sha256.clone();
    let shared_before = blocking(&state, move |db| db.own_track_with_audio(&audio))
        .await?
        .is_some();
    if !shared_before && !api::name_is_findable(&title, ingested.tags.artist.as_deref()) {
        return Err(ApiFailure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name_too_short",
            format!(
                "“{title}” is too short a name for others to find: the title and artist together need at least {} letters or digits. Set them in the song’s tags, or name the file “Artist - Title”.",
                api::MIN_NAME_CHARS
            ),
        ));
    }

    // A picture chosen for the upload replaces the file's own
    if let Some(cover) = cover.or(fallback_cover.filter(|_| ingested.cover_jpeg.is_none())) {
        ingested.cover_jpeg = Some(cover);
    }
    let shared = library::share(&state, ingested)
        .await
        .map_err(ApiFailure::internal)?;
    let track = track_row(&state, shared.id).await?.track;
    if !shared.duplicate {
        tracing::info!(
            id = track.id,
            format = ?track.format,
            size = track.size,
            key = fingerprint(&state.identity.public_hex()),
            "Sharing new track"
        );
    }
    Ok((
        if shared.duplicate {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(UploadResponse {
            track,
            duplicate: shared.duplicate,
        }),
    ))
}

#[derive(Deserialize)]
struct MetadataQuery {
    #[serde(default)]
    refresh: bool,
}
async fn metadata_variants(
    State(state): State<AppState>,
    _: Authorized,
    Path(id): Path<i64>,
    Query(q): Query<MetadataQuery>,
) -> ApiResult<Json<Vec<hopea_common::api::MetadataVariant>>> {
    track_row(&state, id).await?;
    // Bootstrap original attachments for libraries created before variants existed.
    let mut variants = crate::metadata::list(&state, id, q.refresh)
        .await
        .map_err(ApiFailure::internal)?;
    if !variants.iter().any(|v| v.kind == "cover") {
        if let Ok(Some(cover)) = library::original_cover_for_player(&state, id).await {
            let _ = crate::metadata::publish(&state, id, "cover", cover, true).await;
        }
    }
    if !variants.iter().any(|v| v.kind == "lyrics") {
        if let Ok(Some(text)) = crate::lyrics::fetch(&state, id).await {
            let _ = crate::metadata::publish(&state, id, "lyrics", text.into_bytes(), true).await;
        }
    }
    variants = crate::metadata::list(&state, id, false)
        .await
        .map_err(ApiFailure::internal)?;
    Ok(Json(variants))
}
async fn metadata_data(
    State(state): State<AppState>,
    _: Authorized,
    Path((id, hash)): Path<(i64, String)>,
) -> ApiResult<Response> {
    track_row(&state, id).await?;
    let bytes = crate::metadata::data(&state, id, hash)
        .await
        .map_err(ApiFailure::internal)?
        .ok_or_else(ApiFailure::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        bytes,
    )
        .into_response())
}
async fn metadata_select(
    State(state): State<AppState>,
    _: Authorized,
    Path((id, hash)): Path<(i64, String)>,
) -> ApiResult<StatusCode> {
    track_row(&state, id).await?;
    crate::metadata::select(&state, id, hash)
        .await
        .map_err(|_| ApiFailure::not_found())?;
    Ok(StatusCode::NO_CONTENT)
}
