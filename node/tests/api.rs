// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The player's API on a single node, without any network.

use std::path::Path;

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode},
    Router,
};
use hopea_common::api::{self, ApiError, Me, TrackPage, UploadResponse};
use hopea_node::{auth::generate_token, config::Config, AppState};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tower::ServiceExt;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

struct Server {
    _dir: TempDir,
    state: AppState,
    router: Router,
    token: String,
}

impl Server {
    async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    async fn with_config(f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::isolated(dir.path().join("data"));
        f(&mut config);
        let token = generate_token();
        let state = AppState::new(config, &token).await.unwrap();
        let router = state.router();
        Self {
            _dir: dir,
            state,
            router,
            token,
        }
    }

    async fn send(&self, request: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(request).await.unwrap()
    }

    async fn get(&self, token: &str, path: &str) -> Response<Body> {
        self.send(
            Request::get(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn upload(&self, token: &str, name: &str, data: Vec<u8>) -> Response<Body> {
        self.send(
            Request::post(api::tracks_path())
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(api::FILENAME_HEADER, name)
                .header(header::CONTENT_LENGTH, data.len())
                .body(Body::from(data))
                .unwrap(),
        )
        .await
    }
}

async fn json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)))
}

async fn bytes(response: Response<Body>) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn changing_own_cover_keeps_track_and_audio_and_requires_authorization() {
    let server = Server::new().await;
    let uploaded: UploadResponse = json(
        server
            .upload(
                &server.token,
                "Artist%20-%20Song.flac",
                fixture("tone.flac"),
            )
            .await,
    )
    .await;
    let id = uploaded.track.id;
    let before = bytes(server.get(&server.token, &api::audio_path(id)).await).await;
    let mut hashes = Vec::new();
    for color in [[200, 30, 20], [20, 60, 210]] {
        let mut image = Vec::new();
        image::RgbImage::from_pixel(32, 32, image::Rgb(color))
            .write_to(
                &mut std::io::Cursor::new(&mut image),
                image::ImageFormat::Png,
            )
            .unwrap();
        let pending: api::PendingCover = json(
            server
                .send(
                    Request::post(api::covers_path())
                        .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                        .body(Body::from(image))
                        .unwrap(),
                )
                .await,
        )
        .await;
        let body = serde_json::to_vec(&pending).unwrap();
        let denied = server
            .send(
                Request::put(api::cover_path(id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let changed = server
            .send(
                Request::put(api::cover_path(id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(changed.status(), StatusCode::OK);
        let track: api::Track = json(changed).await;
        assert_eq!(track.id, id);
        assert!(track.has_cover);
        hashes.push(bytes(server.get(&server.token, &api::cover_path(id)).await).await);
        assert_eq!(
            bytes(server.get(&server.token, &api::audio_path(id)).await).await,
            before
        );
    }
    assert_ne!(hashes[0], hashes[1]);
    let page: TrackPage = json(server.get(&server.token, &api::tracks_path()).await).await;
    assert_eq!(page.total, 1);
    // A remote track can have local alternatives; malformed pending images still fail.
    let foreign = Server::new().await;
    let (body, signature) = server.state.db.entry_bytes(id).unwrap().unwrap();
    let entry = hopea_node::entry::Entry::from_stored(
        body,
        signature,
        None,
        &foreign.state.config.limits,
        hopea_node::db::now(),
    )
    .unwrap();
    let other_id = foreign
        .state
        .db
        .insert_entry(&entry, hopea_node::db::State::Found)
        .unwrap();
    let refused = foreign
        .send(
            Request::put(api::cover_path(other_id))
                .header(header::AUTHORIZATION, format!("Bearer {}", foreign.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"cover":"anything"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let original = foreign.state.db.entry_bytes(other_id).unwrap().unwrap();
    let pending = hopea_node::library::add_pending_cover(&foreign.state, hashes[0].clone())
        .await
        .unwrap();
    let changed = foreign
        .send(
            Request::put(api::cover_path(other_id))
                .header(header::AUTHORIZATION, format!("Bearer {}", foreign.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&api::PendingCover { cover: pending }).unwrap(),
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        foreign.state.db.entry_bytes(other_id).unwrap().unwrap(),
        original,
        "an alternative must not forge the original signed entry"
    );
}
#[tokio::test]
async fn requires_the_token() {
    let server = Server::new().await;

    let anonymous = server
        .send(Request::get(api::me_path()).body(Body::empty()).unwrap())
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let forged = server.get(&generate_token(), &api::me_path()).await;
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);

    let garbage = server.get("not-a-token", &api::tracks_path()).await;
    assert_eq!(garbage.status(), StatusCode::UNAUTHORIZED);

    let me = server.get(&server.token, &api::me_path()).await;
    assert_eq!(me.status(), StatusCode::OK);
    let me: Me = json(me).await;
    assert_eq!(me.used_bytes, 0);
}

#[tokio::test]
async fn uploads_are_rejected_without_the_token() {
    let server = Server::new().await;
    let response = server
        .send(
            Request::post(api::tracks_path())
                .header(api::FILENAME_HEADER, "tone.flac")
                .body(Body::from(fixture("tone.flac")))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn own_tracks_are_served_as_they_are() {
    let server = Server::new().await;
    let token = server.token.clone();
    let uploaded: UploadResponse = json(
        server
            .upload(&token, "Night%20Drive.flac", fixture("tone.flac"))
            .await,
    )
    .await;
    let track = uploaded.track;

    // FTS syntax in the query is treated as text, not as an error
    let weird = server
        .get(
            &token,
            &format!("{}?q=%22title%3A%20OR%20NEAR(", api::tracks_path()),
        )
        .await;
    assert_eq!(weird.status(), StatusCode::OK);
    let page: TrackPage = json(
        server
            .get(&token, &format!("{}?q=nigh", api::tracks_path()))
            .await,
    )
    .await;
    assert_eq!(page.total, 1);

    let audio = server.get(&token, &api::audio_path(track.id)).await;
    assert_eq!(audio.status(), StatusCode::OK);
    assert_eq!(audio.headers()[header::CONTENT_TYPE], "audio/flac");
    let full = bytes(audio).await;
    assert_eq!(full.len() as u64, track.size);
    assert!(full.starts_with(b"fLaC"));

    let partial = server
        .send(
            Request::get(api::audio_path(track.id))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::RANGE, "bytes=100-199")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes(partial).await, full[100..200]);

    // Stopping sharing removes the track and its file
    let delete = server
        .send(
            Request::delete(api::track_path(track.id))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);
    let gone = server.get(&token, &api::audio_path(track.id)).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejected_uploads_explain_why() {
    let server = Server::new().await;
    let alice = server.token.clone();

    let response = server
        .upload(&alice, "setup.exe", b"MZ\x90\0".repeat(100))
        .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let error: ApiError = json(response).await;
    assert_eq!(error.error, "unsupported_type");

    let response = server
        .upload(&alice, "song.flac", b"MZ\x90\0".repeat(100))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: ApiError = json(response).await;
    assert_eq!(error.error, "not_audio");
    assert!(
        error.message.contains("Windows executable"),
        "{}",
        error.message
    );

    // Video files are refused by their name, and when renamed, by their
    // contents
    let response = server
        .upload(&alice, "clip.mp4", fixture("with-video.mp4"))
        .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let response = server
        .upload(&alice, "clip.m4a", fixture("with-video.mp4"))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Path components in the name are ignored, not used
    let response = server
        .upload(&alice, "..%2F..%2Fetc%2Fpasswd.flac", fixture("tone.flac"))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let uploaded: UploadResponse = json(response).await;
    assert_eq!(uploaded.track.title, "passwd");

    let missing_name = server
        .send(
            Request::post(api::tracks_path())
                .header(header::AUTHORIZATION, format!("Bearer {alice}"))
                .body(Body::from(fixture("tone.flac")))
                .unwrap(),
        )
        .await;
    assert_eq!(missing_name.status(), StatusCode::BAD_REQUEST);

    // Nothing rejected was kept around
    let tmp = server.state.storage.tmp_dir();
    assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
}

#[tokio::test]
async fn enforces_size_and_quota() {
    let server = Server::with_config(|c| {
        c.limits.max_upload_bytes = 20 * 1024;
        c.quota_bytes = 16 * 1024;
    })
    .await;
    let alice = server.token.clone();

    let too_big = server.upload(&alice, "tone.wav", fixture("tone.wav")).await;
    assert_eq!(too_big.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let first = server
        .upload(&alice, "Steady Tone.flac", fixture("tone.flac"))
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let over_quota = server.upload(&alice, "tone.m4a", fixture("tone.m4a")).await;
    assert_eq!(over_quota.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let error: ApiError = json(over_quota).await;
    assert_eq!(error.error, "quota_exceeded");
}

#[tokio::test]
async fn hostile_upload_lengths_are_checked_against_actual_chunks() {
    let server = Server::with_config(|c| {
        c.limits.max_upload_bytes = 1024;
        c.quota_bytes = 4096;
    })
    .await;
    // Exercise the handler directly so a well-behaved HTTP client cannot
    // silently correct the attacker's false Content-Length for this test.
    for (declared, chunks, expected) in [
        (Some(1), vec![512, 512], StatusCode::BAD_REQUEST),
        (Some(1000), vec![12], StatusCode::BAD_REQUEST),
        (None, vec![600, 600], StatusCode::PAYLOAD_TOO_LARGE),
        (None, vec![2048], StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let body = Body::from_stream(futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|n| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0u8; n]))),
        ));
        let mut request = Request::post(api::tracks_path())
            .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
            .header(api::FILENAME_HEADER, "test.wav");
        if let Some(length) = declared {
            request = request.header(header::CONTENT_LENGTH, length);
        }
        assert_eq!(
            server.send(request.body(body).unwrap()).await.status(),
            expected
        );
        assert_eq!(
            std::fs::read_dir(server.state.storage.tmp_dir())
                .unwrap()
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn upload_without_length_cannot_exceed_disk_quota() {
    let server = Server::with_config(|c| {
        c.limits.max_upload_bytes = 4096;
        c.quota_bytes = 1024;
    })
    .await;
    let body = Body::from_stream(futures_util::stream::iter(
        [512, 513].map(|n| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0u8; n]))),
    ));
    let response = server
        .send(
            Request::post(api::tracks_path())
                .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                .header(api::FILENAME_HEADER, "test.wav")
                .body(body)
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json::<ApiError>(response).await.error, "quota_exceeded");
    assert_eq!(
        std::fs::read_dir(server.state.storage.tmp_dir())
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn unknown_routes_are_not_found() {
    let server = Server::new().await;
    let alice = server.token.clone();
    let response = server.get(&alice, "/api/v1/../../catalog.db").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = server.get(&alice, "/api/v1/tracks/abc").await;
    assert!(response.status().is_client_error());
}

#[tokio::test]
async fn sharing_is_never_limited_far_below_downloading() {
    use hopea_common::api::SpeedLimits;
    let server = Server::new().await;
    let token = server.token.clone();
    let put = |limits: SpeedLimits| {
        Request::put(api::limits_path())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&limits).unwrap()))
            .unwrap()
    };
    let mib = 1024 * 1024;

    // A tiny sharing limit is raised to a seventh of the download limit
    let applied: SpeedLimits = json(
        server
            .send(put(SpeedLimits {
                download: Some(7 * mib),
                upload: Some(1),
            }))
            .await,
    )
    .await;
    assert_eq!(applied.download, Some(7 * mib));
    assert_eq!(applied.upload, Some(mib));
    let now: SpeedLimits = json(server.get(&token, &api::limits_path()).await).await;
    assert_eq!(now, applied);

    // Sharing can't be limited while downloading isn't
    let applied: SpeedLimits = json(
        server
            .send(put(SpeedLimits {
                download: None,
                upload: Some(mib),
            }))
            .await,
    )
    .await;
    assert_eq!(
        applied,
        SpeedLimits {
            download: None,
            upload: None
        }
    );
}

#[tokio::test]
async fn gaming_restores_requested_limits_including_edits_made_while_active() {
    use hopea_common::api::SpeedLimits;
    let server = Server::new().await;
    let torrents = &server.state.torrents;
    let original = torrents.limits();
    torrents.set_gaming(true);
    assert!(torrents.gaming());
    assert_eq!(torrents.limits(), original.during_game());
    torrents.set_gaming(true);
    torrents.set_gaming(false);
    assert!(!torrents.gaming());
    assert_eq!(torrents.limits(), original);

    torrents.set_gaming(true);
    for edited in [
        SpeedLimits {
            download: Some(64 * 1024),
            upload: Some(32 * 1024),
        },
        SpeedLimits {
            download: Some(10 * 1024 * 1024),
            upload: Some(2 * 1024 * 1024),
        },
        SpeedLimits {
            download: None,
            upload: None,
        },
    ] {
        let response = server
            .send(
                Request::put(api::limits_path())
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&edited).unwrap()))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let applied: SpeedLimits = json(response).await;
        assert_eq!(applied, edited.during_game());
        torrents.set_gaming(false);
        assert_eq!(torrents.limits(), edited.fair());
        torrents.set_gaming(true);
    }
    torrents.stop().await;
}
