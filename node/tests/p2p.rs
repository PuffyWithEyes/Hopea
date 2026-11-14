// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Real nodes talking to each other over loopback: sharing, searching,
//! streaming over BitTorrent, verifying downloads, favorites and the feed.

use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use hopea_common::api::{
    self, ApiError, Me, Reachability, Track, TrackPage, Transfer, UploadResponse,
};
use hopea_node::{auth::generate_token, config::Config, ingest, Node};
use reqwest::{header, StatusCode};
use serde::de::DeserializeOwned;
use tempfile::TempDir;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

struct TestNode {
    _dir: TempDir,
    node: Node,
    token: String,
    http: reqwest::Client,
}

impl TestNode {
    async fn start(name: &str, peers: Vec<SocketAddr>) -> Self {
        Self::start_with(name, |c| c.peers = peers).await
    }

    async fn start_with(name: &str, f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::isolated(dir.path().join("data"));
        config.name = Some(name.into());
        f(&mut config);
        let token = generate_token();
        let node = Node::start(config, &token).await.unwrap();
        Self {
            _dir: dir,
            node,
            token,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.node.api_addr)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(self.url(path)).bearer_auth(&self.token)
    }

    async fn json<T: DeserializeOwned>(&self, path: &str) -> T {
        let response = self.get(path).send().await.unwrap();
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        response.json().await.unwrap()
    }

    async fn share(&self, name: &str, body: Vec<u8>) -> reqwest::Response {
        self.http
            .post(self.url(&api::tracks_path()))
            .bearer_auth(&self.token)
            .header(api::FILENAME_HEADER, name)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    async fn search(&self, q: &str, network: bool) -> TrackPage {
        self.json(&format!("{}?q={q}&network={network}", api::tracks_path()))
            .await
    }

    async fn wait_until_local(&self, id: i64) -> Track {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let track: Track = self.json(&api::track_path(id)).await;
            if track.local {
                return track;
            }
            assert!(Instant::now() < deadline, "the download never completed");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires LAN multicast; run explicitly on a host network"]
async fn lan_discovers_searches_and_downloads_without_dht_or_configured_peers() {
    let configure = |c: &mut Config| {
        c.local_discovery = true;
        c.p2p_listen = "0.0.0.0:0".parse().unwrap();
    };
    let alice = TestNode::start_with("lan-alice", configure).await;
    let bob = TestNode::start_with("lan-bob", configure).await;
    assert!(bob.node.state.config.peers.is_empty());
    assert!(!bob.node.state.config.dht);
    wait_for("mDNS discovery of Alice", || {
        bob.node
            .state
            .lan
            .addresses()
            .iter()
            .any(|a| a.port() == alice.node.p2p_addr.port())
    })
    .await;
    assert!(!bob
        .node
        .state
        .lan
        .addresses()
        .iter()
        .any(|a| a.port() == bob.node.p2p_addr.port()));

    let name = format!("lancheck{}.flac", alice.node.p2p_addr.port());
    let response = alice.share(&name, fixture("tone.flac")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let shared: UploadResponse = response.json().await.unwrap();
    let found = bob.search(name.trim_end_matches(".flac"), true).await;
    assert_eq!(found.total, 1);
    let track = &found.tracks[0];
    assert_eq!(track.sha256, shared.track.sha256);
    let audio = bob.get(&api::audio_path(track.id)).send().await.unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    assert_eq!(&audio.bytes().await.unwrap()[..4], b"RIFF");
    assert_eq!(
        bob.wait_until_local(track.id).await.sha256,
        shared.track.sha256
    );

    let port = alice.node.p2p_addr.port();
    alice.node.shutdown().await;
    wait_for("mDNS goodbye after sharing stops", || {
        !bob.node
            .state
            .lan
            .addresses()
            .iter()
            .any(|a| a.port() == port)
    })
    .await;
    bob.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv4_only_shares_and_downloads_without_ipv6_connections() {
    let alice = TestNode::start("alice", vec![]).await;
    let ipv6 = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let ipv6_addr = ipv6.local_addr().unwrap();
    let bob = TestNode::start_with("bob", |config| {
        config.ipv4_only = true;
        config.p2p_listen = "[::]:0".parse().unwrap();
        config.peers = vec![alice.node.p2p_addr, ipv6_addr];
        config.relays = vec![ipv6_addr];
        config.upnp = true;
        config.local_discovery = true;
    })
    .await;
    assert!(bob.node.p2p_addr.is_ipv4());
    assert_eq!(bob.node.state.config.peers, vec![alice.node.p2p_addr]);
    assert!(bob.node.state.config.relays.is_empty());
    assert!(!bob.node.state.config.upnp && !bob.node.state.config.local_discovery);
    // Even an old stored IPv6 source cannot bypass the client's family limit.
    let request = bob
        .node
        .state
        .http
        .get(format!("http://{ipv6_addr}/"))
        .timeout(Duration::from_millis(500))
        .send()
        .await;
    assert!(request.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), ipv6.accept())
            .await
            .is_err()
    );
    let response = alice
        .share("Night%20Drive.flac", fixture("tone.flac"))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let found = bob.search("night", true).await;
    assert_eq!(found.total, 1);
    let id = found.tracks[0].id;
    let audio = bob.get(&api::audio_path(id)).send().await.unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    let wav = audio.bytes().await.unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert!(bob.wait_until_local(id).await.local);
    bob.node.shutdown().await;
    alice.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_find_stream_and_pass_on() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;

    // The API needs the token
    let anonymous = bob
        .http
        .get(bob.url(&api::tracks_path()))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let response = alice
        .share("Night%20Drive.flac", fixture("tone.flac"))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let shared: UploadResponse = response.json().await.unwrap();
    assert!(shared.track.mine && shared.track.local);
    assert!(shared.track.uploader.starts_with("alice · "));
    let me: Me = alice.json(&api::me_path()).await;
    assert_eq!(me.name, shared.track.uploader);
    assert_eq!(me.used_bytes, shared.track.size);

    // The same audio again is a duplicate
    let again: UploadResponse = alice
        .share("copy.flac", fixture("tone.flac"))
        .await
        .json()
        .await
        .unwrap();
    assert!(again.duplicate);
    assert_eq!(again.track.id, shared.track.id);

    // Search answers are small: no torrent in them, only its hashes
    let raw = bob
        .http
        .get(format!(
            "http://{}{}?q=night",
            alice.node.p2p_addr,
            hopea_node::p2p::SEARCH_PATH
        ))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(answer["entries"].as_array().unwrap().len(), 1);
    assert!(answer["entries"][0].get("info").is_none());
    assert!(raw.len() < 2000, "{} bytes for one result", raw.len());

    // Bob knows nothing until he searches the network
    assert_eq!(bob.search("night", false).await.total, 0);
    let page = bob.search("night%20drive", true).await;
    assert_eq!(page.nodes_answered, 1);
    assert_eq!(page.total, 1);
    let found = &page.tracks[0];
    assert_eq!(found.title, "Night Drive");
    assert_eq!(found.uploader, shared.track.uploader);
    assert!(!found.mine && !found.local);
    // Now it is remembered
    assert_eq!(bob.search("night", false).await.total, 1);

    // A search that matches nothing on Alice's node finds nothing
    assert_eq!(bob.search("zzzz", true).await.total, 0);

    // Bob streams it: decoded on his node and served as PCM, never the file
    let audio = bob.get(&api::audio_path(found.id)).send().await.unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    assert_eq!(audio.headers()[header::CONTENT_TYPE], "audio/wav");
    assert_eq!(audio.headers()[header::ACCEPT_RANGES], "bytes");
    let wav = audio.bytes().await.unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let channels = u16::from_le_bytes([wav[22], wav[23]]) as usize;
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]) as usize;
    let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
    assert_eq!(wav.len(), 44 + data_len);
    // One second of tone
    let frames = data_len / (2 * channels);
    assert!(
        (rate * 95 / 100..=rate * 105 / 100).contains(&frames),
        "{frames} frames at {rate} Hz"
    );
    // And it is audible, not silence padding
    let loud = wav[44..]
        .chunks_exact(2)
        .filter(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 1000)
        .count();
    assert!(loud > frames / 4);

    // Seeking is a range request, and gives the same bytes
    let middle = 44 + (frames / 2) * 2 * channels;
    let ranged = bob
        .get(&api::audio_path(found.id))
        .header(header::RANGE, format!("bytes={middle}-{}", middle + 999))
        .send()
        .await
        .unwrap();
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        ranged.headers()[header::CONTENT_RANGE],
        format!("bytes {middle}-{}/{}", middle + 999, wav.len())
    );
    let part = ranged.bytes().await.unwrap();
    assert_eq!(part.len(), 1000);
    assert_eq!(&part[..], &wav[middle..middle + 1000]);

    // The whole file arrives, passes validation, and is kept
    let held = bob.wait_until_local(found.id).await;
    assert!(!held.mine);
    let peaks: Vec<(f32, f32)> = bob.json(&api::peaks_path(found.id)).await;
    assert!(peaks.len() >= 4);
    // Only the publisher can remove a track from the network
    let delete = bob
        .http
        .delete(bob.url(&api::track_path(found.id)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::FORBIDDEN);

    // Carol only knows Bob, and finds the track there, still signed by Alice
    let carol = TestNode::start("carol", vec![bob.node.p2p_addr]).await;
    let page = carol.search("drive", true).await;
    assert_eq!(page.total, 1);
    assert_eq!(page.tracks[0].uploader, shared.track.uploader);
    alice.node.shutdown().await;
    let wav_from_bob = carol
        .get(&api::audio_path(page.tracks[0].id))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(wav_from_bob, wav);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn favorites_and_recommendations() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;

    // Alice shares two tracks of different genres. Tags come from the
    // files; the fixtures have none, so the genre is given with an MP3's
    // ID3 tag written here.
    for (fixture_name, name, genre) in [
        ("tone.mp3", "Heavy.mp3", "Nu Metal"),
        ("tone.ogg", "Calm.ogg", "Jazz"),
    ] {
        let file = tagged(fixture_name, name, genre);
        let response = alice.share(name, file).await;
        assert_eq!(response.status(), StatusCode::CREATED, "{name}");
    }
    // Bob's own taste: a nu metal track he shares himself
    let response = bob
        .share("Mine.flac", tagged("tone.flac", "Mine.flac", "Nu Metal"))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let feed: TrackPage = bob
        .json(&format!("{}?refresh=true", api::recommendations_path()))
        .await;
    let titles: Vec<_> = feed.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Heavy"], "only the matching genre is recommended");

    // Adding it to favorites downloads and keeps it
    let heavy = feed.tracks[0].id;
    let favorite: Track = bob
        .http
        .put(bob.url(&api::favorite_path(heavy)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(favorite.favorite);
    bob.wait_until_local(heavy).await;
    // Sharing a local song does not silently add it to Favorites
    let favorites: TrackPage = bob.json(&api::favorites_path()).await;
    let titles: Vec<_> = favorites.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Heavy"]);
    assert!(favorites.tracks.iter().all(|t| t.favorite));

    // Favorites are not recommended again
    let feed: TrackPage = bob.json(&api::recommendations_path()).await;
    assert!(feed.tracks.iter().all(|t| t.id != heavy));

    // Unstarring keeps the local copy and its automatic sharing.
    for id in [heavy] {
        let response = bob
            .http
            .delete(bob.url(&api::favorite_path(id)))
            .bearer_auth(&bob.token)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
    let track: Track = bob.json(&api::track_path(heavy)).await;
    assert!(!track.favorite && track.local);
    let favorites: TrackPage = bob.json(&api::favorites_path()).await;
    let titles: Vec<_> = favorites.tracks.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.is_empty());

    let missing = bob
        .http
        .put(bob.url(&api::favorite_path(9999)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let error: ApiError = missing.json().await.unwrap();
    assert_eq!(error.error, "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recommendations_forget_offline_sources_without_another_dht_search() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    assert_eq!(
        alice
            .share("Heavy.mp3", tagged("tone.mp3", "Heavy.mp3", "Nu Metal"))
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        bob.share("Mine.flac", tagged("tone.flac", "Mine.flac", "Nu Metal"))
            .await
            .status(),
        StatusCode::CREATED
    );
    let feed: TrackPage = bob
        .json(&format!("{}?refresh=true", api::recommendations_path()))
        .await;
    assert_eq!(feed.tracks.len(), 1);
    let id = feed.tracks[0].id;
    drop(alice);
    let gone = bob.get(&api::audio_path(id)).send().await.unwrap();
    assert_eq!(
        gone.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an offline source is not an internal server error"
    );
    assert_eq!(
        gone.json::<ApiError>().await.unwrap().error,
        "track_unavailable"
    );
    assert!(bob.node.state.db.sources(id).unwrap().is_empty());
    let prefetch = bob
        .http
        .post(bob.url(&api::prefetch_path(id)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert_eq!(prefetch.status(), StatusCode::SERVICE_UNAVAILABLE);
    let feed: TrackPage = bob.json(&api::recommendations_path()).await;
    assert!(
        feed.tracks.is_empty(),
        "cached recommendations must not retain a disconnected source"
    );
    let ids: Vec<i64> = bob
        .json(&format!(
            "{}/available?ids={id}&playback=true",
            api::tracks_path()
        ))
        .await;
    assert!(
        ids.is_empty(),
        "the active recommendation queue must forget it too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lyrics_are_signed_updated_and_forwarded_after_publisher_disconnects() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let shared: UploadResponse = alice
        .share("Lyrics%20Artist%20-%20Song.flac", fixture("tone.flac"))
        .await
        .json()
        .await
        .unwrap();
    let alice_path = format!("{}/lyrics", api::track_path(shared.track.id));
    let text = "[00:00.00]First line\n[00:00.50]Second line";
    assert_eq!(
        alice
            .http
            .put(alice.url(&alice_path))
            .bearer_auth(&alice.token)
            .body(text)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let page = bob.search("lyrics", true).await;
    let id = page.tracks[0].id;
    let bob_path = format!("{}/lyrics", api::track_path(id));
    // Fetching audio also starts the signed attachment download.
    let bytes = bob
        .get(&api::audio_path(id))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(!bytes.is_empty());
    bob.wait_until_local(id).await;
    assert_eq!(
        bob.json::<Option<String>>(&bob_path).await.as_deref(),
        Some(text)
    );
    // Alternatives may be published, but cannot forge the publisher's legacy signature.
    assert!(
        hopea_node::lyrics::publish(&bob.node.state, id, "forged".into())
            .await
            .is_err()
    );
    let updated = "[00:00.00]Updated lyrics";
    assert_eq!(
        alice
            .http
            .put(alice.url(&alice_path))
            .bearer_auth(&alice.token)
            .body(updated)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        bob.json::<Option<String>>(&bob_path).await.as_deref(),
        Some(updated)
    );
    // Tampering with a cached signed attachment cannot change its text.
    let hash = bob.node.state.db.torrent(id).unwrap().unwrap().info_hash;
    let path = bob.node.state.storage.lyrics_path(&hash);
    let valid = std::fs::read(&path).unwrap();
    let bad = String::from_utf8(valid.clone())
        .unwrap()
        .replace("Updated lyrics", "Forged lyrics");
    std::fs::write(&path, bad).unwrap();
    assert_eq!(
        bob.json::<Option<String>>(&bob_path).await.as_deref(),
        Some(updated)
    );
    assert_eq!(
        alice
            .http
            .put(alice.url(&alice_path))
            .bearer_auth(&alice.token)
            .body(vec![b'x'; 256 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        alice
            .http
            .put(alice.url(&alice_path))
            .bearer_auth(&alice.token)
            .body("bad\0lyrics")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    drop(alice);
    let charlie = TestNode::start("charlie", vec![bob.node.p2p_addr]).await;
    let page = charlie.search("lyrics", true).await;
    assert_eq!(page.tracks.len(), 1);
    let charlie_path = format!("{}/lyrics", api::track_path(page.tracks[0].id));
    assert_eq!(
        charlie
            .json::<Option<String>>(&charlie_path)
            .await
            .as_deref(),
        Some(updated)
    );
    assert_eq!(
        page.tracks[0].sha256, shared.track.sha256,
        "lyrics must not change audio identity"
    );
}

/// A fixture with a genre, title and artist in its usual kind of tag.
fn tagged(fixture_name: &str, name: &str, genre: &str) -> Vec<u8> {
    use lofty::{config::WriteOptions, file::TaggedFileExt, prelude::*, tag::Tag};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, fixture(fixture_name)).unwrap();
    let mut file = lofty::read_from_path(&path).unwrap();
    let tag_type = file.primary_tag_type();
    file.insert_tag(Tag::new(tag_type));
    let tag = file.primary_tag_mut().unwrap();
    tag.set_genre(genre.into());
    tag.set_title(name.rsplit_once('.').unwrap().0.into());
    tag.set_artist(format!("{genre} Band"));
    file.save_to_path(&path, WriteOptions::default()).unwrap();
    std::fs::read(&path).unwrap()
}

#[test]
fn validated_files_validate_again_unchanged() {
    let limits = Config::new(Default::default()).limits;
    for name in [
        "tone.flac",
        "tone.mp3",
        "tone.m4a",
        "tone-alac.m4a",
        "tone.ogg",
        "tone.wav",
    ] {
        let work = tempfile::tempdir().unwrap();
        let input = work.path().join(name);
        std::fs::write(&input, fixture(name)).unwrap();
        let first = ingest::ingest(&input, name, work.path(), &limits).unwrap();
        let format = first.format;

        // What a downloading node does with what a sharing node produced
        let downloaded = work.path().join("downloaded");
        std::fs::copy(&first.file, &downloaded).unwrap();
        let second = ingest::verify_download(
            &downloaded,
            format,
            &first.sha256,
            &first.audio_sha256,
            first.duration_ms,
            work.path(),
            &limits,
        );
        assert!(second.is_ok(), "{name}: {:?}", second.err());
        assert!(
            ingest::verify_download(
                &downloaded,
                format,
                &first.sha256,
                &first.audio_sha256,
                first.duration_ms + 60_000,
                work.path(),
                &limits
            )
            .is_err(),
            "a real audio hash must not authenticate a forged duration"
        );

        // Anything the rebuild would have dropped is refused, even when the
        // hash is what the entry claims: here a trailing ID3v1 tag (stripped
        // from MP3s, and trailing junk to every other format)
        let mut bytes = std::fs::read(&downloaded).unwrap();
        bytes.extend_from_slice(b"TAG");
        bytes.extend_from_slice(&[b'x'; 125]);
        std::fs::write(&downloaded, &bytes).unwrap();
        let sha = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(&bytes))
        };
        let tampered = ingest::verify_download(
            &downloaded,
            format,
            &sha,
            &first.audio_sha256,
            first.duration_ms,
            work.path(),
            &limits,
        );
        assert!(tampered.is_err(), "{name}: appended data was accepted");
    }
}

async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreachable_nodes_share_through_a_relay() {
    // Alice can be reached, and nobody else to ask says otherwise
    let alice = TestNode::start("alice", vec![]).await;
    wait_for("alice to be reachable", || {
        alice.node.state.reach.get() == Reachability::Direct
    })
    .await;

    // Carol can't be reached; Alice relays for her
    let carol = TestNode::start_with("carol", |c| {
        c.force_relay = true;
        c.relays = vec![alice.node.p2p_addr];
    })
    .await;
    wait_for("carol to use the relay", || {
        carol.node.state.reach.get() == Reachability::Relay
    })
    .await;
    let me: Me = carol.json(&api::me_path()).await;
    assert_eq!(me.reachability, Reachability::Relay);
    assert_eq!(alice.node.state.relay.clients(), 1);

    // Carol shares after registering: the relay must learn the new words
    let response = carol
        .share("Night%20Drive.flac", fixture("tone.flac"))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let published: UploadResponse = response.json().await.unwrap();
    let lyrics = "[00:00.00]Lyrics through the relay";
    assert_eq!(
        carol
            .http
            .put(carol.url(&format!("{}/lyrics", api::track_path(published.track.id))))
            .bearer_auth(&carol.token)
            .body(lyrics)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    // Bob only knows Alice, and finds Carol's track through her
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let mut page = bob.search("night", true).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while page.total == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        page = bob.search("night", true).await;
    }
    assert_eq!(page.total, 1);
    let track = page.tracks[0].clone();
    assert!(track.uploader.starts_with("carol · "), "{}", track.uploader);

    // Bob plays it: Alice fetches it from Carol, checks it, and shares it
    let audio = bob.get(&api::audio_path(track.id)).send().await.unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    let wav = audio.bytes().await.unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    bob.wait_until_local(track.id).await;

    let received: Option<String> = bob
        .json(&format!("{}/lyrics", api::track_path(track.id)))
        .await;
    assert_eq!(received.as_deref(), Some(lyrics));

    let relay_variants: Vec<hopea_common::api::MetadataVariant> = bob
        .json(&format!(
            "{}/variants?refresh=true",
            api::track_path(track.id)
        ))
        .await;
    assert!(
        relay_variants
            .iter()
            .any(|v| v.kind == "lyrics" && v.peers >= 1),
        "the relay also serves metadata variants"
    );

    // Alice kept a verified copy and shares it on, still signed by Carol
    let at_alice = alice.search("night", false).await;
    assert_eq!(at_alice.total, 1);
    assert!(at_alice.tracks[0].local);
    assert_eq!(at_alice.tracks[0].uploader, track.uploader);

    // A relay only takes uploads it asked for, from the client that offered
    let stray = bob
        .http
        .post(format!(
            "http://{}{}?session=nope&info_hash={}",
            alice.node.p2p_addr,
            hopea_node::relay::UPLOAD_PATH,
            "0".repeat(40)
        ))
        .body(fixture("tone.flac"))
        .send()
        .await
        .unwrap();
    assert_eq!(stray.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reachability_is_checked_by_other_nodes() {
    let alice = TestNode::start("alice", vec![]).await;
    // Bob asks Alice to connect back to him, which works on loopback
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    wait_for("bob to be found reachable", || {
        bob.node.state.reach.get() == Reachability::Direct
    })
    .await;

    // Asking to be reached on a port where nothing listens finds nobody
    let unused = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let answer: serde_json::Value = bob
        .http
        .get(format!(
            "http://{}{}?port={unused}",
            alice.node.p2p_addr,
            hopea_node::reach::DIALBACK_PATH
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer["node"], serde_json::Value::Null);
}

/// A fixture tagged as a track of an album.
fn album_track(fixture_name: &str, title: &str, album: &str, number: u32) -> Vec<u8> {
    use lofty::{config::WriteOptions, file::TaggedFileExt, prelude::*, tag::Tag};
    let dir = tempfile::tempdir().unwrap();
    let ext = fixture_name.rsplit_once('.').unwrap().1;
    let path = dir.path().join(format!("track.{ext}"));
    std::fs::write(&path, fixture(fixture_name)).unwrap();
    let mut file = lofty::read_from_path(&path).unwrap();
    let tag_type = file.primary_tag_type();
    file.insert_tag(Tag::new(tag_type));
    let tag = file.primary_tag_mut().unwrap();
    tag.set_title(title.into());
    tag.set_artist("Linkin Park".into());
    tag.set_album(album.into());
    tag.set_track(number);
    file.save_to_path(&path, WriteOptions::default()).unwrap();
    std::fs::read(&path).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whole_albums_are_found_and_saved() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;

    // Shared out of order, from an album folder, plus a song from another
    for (fixture_name, title, album, number) in [
        ("tone.ogg", "Numb", "Meteora", 13),
        ("tone.flac", "Foreword", "Meteora", 1),
        ("tone.mp3", "Faint", "Meteora", 7),
        ("tone.wav", "Papercut", "Hybrid Theory", 1),
    ] {
        let ext = fixture_name.rsplit_once('.').unwrap().1;
        let response = alice
            .share(
                &format!("{title}.{ext}"),
                album_track(fixture_name, title, album, number),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED, "{title}");
    }

    let page: TrackPage = bob
        .json(&format!(
            "{}?album=meteora&artist=Linkin%20Park&network=true",
            api::album_path()
        ))
        .await;
    assert_eq!(page.nodes_answered, 1);
    let titles: Vec<_> = page.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Foreword", "Faint", "Numb"]);

    // A file is only handed out once it is here and verified
    let numb = page.tracks[2].id;
    let early = bob.get(&api::file_path(numb)).send().await.unwrap();
    assert_eq!(early.status(), StatusCode::CONFLICT);

    let favorite = bob
        .http
        .put(bob.url(&api::favorite_path(numb)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert!(favorite.status().is_success());
    bob.wait_until_local(numb).await;
    let saved = bob.get(&api::file_path(numb)).send().await.unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(saved.headers()[header::CONTENT_TYPE], "audio/ogg");
    let bytes = saved.bytes().await.unwrap();
    assert_eq!(bytes.len() as u64, page.tracks[2].size);
    assert!(bytes.starts_with(b"OggS"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn short_titles_are_found_by_file_name() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;

    // An untagged file named "A" is no name anyone finds: it isn't shared
    let refused = alice.share("A.mp3", fixture("tone.mp3")).await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: ApiError = refused.json().await.unwrap();
    assert_eq!(error.error, "name_too_short");

    // With its artist, the same short title is enough
    let shared: UploadResponse = alice
        .share("A.mp3", album_track("tone.mp3", "A", "Meteora", 1))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(shared.track.title, "A");

    for query in ["A.mp3", "a"] {
        let page = bob.search(query, true).await;
        assert_eq!(page.nodes_answered, 1, "{query}");
        assert_eq!(page.total, 1, "{query}");
        assert_eq!(page.tracks[0].title, "A");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn searches_forgive_typos_and_partial_words() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let response = alice
        .share("Numb.ogg", album_track("tone.ogg", "Numb", "Meteora", 13))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    for query in ["linkn%20prk", "link", "meteor", "nmub"] {
        let page = bob.search(query, true).await;
        assert_eq!(page.total, 1, "{query}");
        assert_eq!(page.tracks[0].title, "Numb", "{query}");
    }
    let page = bob.search("metallica", true).await;
    assert_eq!(page.total, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cover_art_given_when_sharing_reaches_others() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let add_cover = |body: Vec<u8>| {
        alice
            .http
            .post(alice.url(&api::covers_path()))
            .bearer_auth(&alice.token)
            .body(body)
            .send()
    };
    let share_with_cover = |name: &str, cover: &str| {
        alice
            .http
            .post(alice.url(&api::tracks_path()))
            .bearer_auth(&alice.token)
            .header(api::FILENAME_HEADER, name)
            .header(api::COVER_HEADER, cover)
            .body(fixture("tone.flac"))
            .send()
    };

    // Not a picture
    let refused = add_cover(b"MZ\x90\0 not an image".to_vec()).await.unwrap();
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // A picture nobody sent
    let gone = share_with_cover("Night%20Drive.flac", &"0".repeat(64))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: ApiError = gone.json().await.unwrap();
    assert_eq!(error.error, "cover_gone");

    // A small PNG, given when sharing
    let mut png = Vec::new();
    image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 90]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let pending: api::PendingCover = add_cover(png).await.unwrap().json().await.unwrap();
    let shared: UploadResponse = share_with_cover("Night%20Drive.flac", &pending.cover)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(shared.track.has_cover);
    assert!(shared.track.mine && !shared.track.favorite);

    // An artwork update requires JSON naming a validated pending picture.
    let changed = alice
        .http
        .put(alice.url(&api::cover_path(shared.track.id)))
        .bearer_auth(&alice.token)
        .body(b"anything".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Bob finds it with its cover, which he gets re-encoded as a JPEG
    let page = bob.search("night", true).await;
    assert_eq!(page.total, 1);
    assert!(page.tracks[0].has_cover);
    let cover = bob
        .get(&api::cover_path(page.tracks[0].id))
        .send()
        .await
        .unwrap();
    assert_eq!(cover.status(), StatusCode::OK);
    let jpeg = cover.bytes().await.unwrap();
    assert!(jpeg.starts_with(&[0xFF, 0xD8]));
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (64, 64));

    // Bob only shares tracks he has the file of, so Carol finds it at
    // Alice's and not his, but the cover reaches her the same way
    let carol = TestNode::start("carol", vec![alice.node.p2p_addr]).await;
    let page = carol.search("night", true).await;
    assert!(page.tracks[0].has_cover);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_song_downloads_ahead_except_while_gaming() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let response = alice
        .share(
            "Next Song.flac",
            album_track("tone.flac", "Next Song", "Ahead", 2),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let page = bob.search("next song", true).await;
    let id = page.tracks[0].id;
    assert_eq!(page.tracks[0].sources, 1);

    let before: Transfer = bob.json(&api::transfer_path(id)).await;
    assert_eq!(before.progress, 0.0);
    bob.node.state.torrents.set_gaming(true);
    let deferred = bob
        .http
        .post(bob.url(&api::prefetch_path(id)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert_eq!(deferred.status(), StatusCode::NO_CONTENT);
    let during: Transfer = bob.json(&api::transfer_path(id)).await;
    assert_eq!(during.progress, 0.0);
    let torrent = bob.node.state.db.torrent(id).unwrap().unwrap();
    assert!(bob.node.state.torrents.get(&torrent.info_hash).is_none());
    bob.node.state.torrents.set_gaming(false);
    let started = bob
        .http
        .post(bob.url(&api::prefetch_path(id)))
        .bearer_auth(&bob.token)
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    let track = bob.wait_until_local(id).await;
    assert!(!track.favorite, "fetched ahead, not starred");
    let after: Transfer = bob.json(&api::transfer_path(id)).await;
    assert_eq!(after.progress, 1.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gaming_keeps_explicit_playback_available() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let response = alice.share("Gaming Song.flac", fixture("tone.flac")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    bob.node.state.torrents.set_gaming(true);
    let page = bob.search("gaming", true).await;
    assert_eq!(page.tracks.len(), 1);
    let id = page.tracks[0].id;
    let audio = bob.get(&api::audio_path(id)).send().await.unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    assert_eq!(&audio.bytes().await.unwrap()[..4], b"RIFF");
    assert!(bob.wait_until_local(id).await.local);
    assert!(bob.node.state.torrents.gaming());
    bob.node.shutdown().await;
    alice.node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_rechecks_offline_sources_and_recovers_missing_cached_artwork() {
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let mut image = Vec::new();
    image::RgbImage::from_pixel(32, 32, image::Rgb([20, 150, 90]))
        .write_to(
            &mut std::io::Cursor::new(&mut image),
            image::ImageFormat::Png,
        )
        .unwrap();
    let pending: api::PendingCover = alice
        .http
        .post(alice.url(&api::covers_path()))
        .bearer_auth(&alice.token)
        .body(image)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uploaded = alice
        .http
        .post(alice.url(&api::tracks_path()))
        .bearer_auth(&alice.token)
        .header(api::FILENAME_HEADER, "Twilight%20Artist%20-%20Song.flac")
        .header(api::COVER_HEADER, pending.cover)
        .body(fixture("tone.flac"))
        .send()
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let query = format!(
        "{}?q=twilight&network=true&discovery=true",
        api::tracks_path()
    );
    let page: TrackPage = bob.json(&query).await;
    assert_eq!(page.tracks.len(), 1);
    let id = page.tracks[0].id;
    let cover_path = api::cover_path(id);
    assert!(bob
        .get(&cover_path)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let info_hash = bob.node.state.db.torrent(id).unwrap().unwrap().info_hash;
    let cached = bob.node.state.storage.cover_path(&info_hash);
    assert!(cached.is_file());
    std::fs::remove_file(&cached).unwrap();
    assert!(bob.node.state.db.cover_state(id).unwrap().unwrap().here);
    // This used to return 500/ENOENT because the stale database flag was trusted.
    let recovered = bob.get(&cover_path).send().await.unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    assert!(cached.is_file());
    drop(alice);
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::fs::remove_file(&cached).unwrap();
    assert_eq!(
        bob.get(&cover_path).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert!(!bob.node.state.db.cover_state(id).unwrap().unwrap().here);
    let available: Vec<i64> = bob
        .json(&format!(
            "{}/available?ids={id}&q=twilight",
            api::tracks_path()
        ))
        .await;
    assert!(
        available.is_empty(),
        "periodic refresh must remove the disconnected source"
    );
    let page: TrackPage = bob.json(&query).await;
    assert!(page.tracks.is_empty());
    assert_eq!(page.total, 0);
    // The metadata is retained for later rediscovery; user data isn't deleted.
    assert_eq!(bob.search("twilight", false).await.total, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_keeps_a_song_while_another_verified_copy_is_online() {
    let alice = TestNode::start("alice", vec![]).await;
    assert_eq!(
        alice
            .share("Twilight%20Artist.flac", fixture("tone.flac"))
            .await
            .status(),
        StatusCode::CREATED
    );
    let charlie = TestNode::start("charlie", vec![alice.node.p2p_addr]).await;
    let song = charlie.search("twilight", true).await.tracks.remove(0);
    assert!(charlie
        .http
        .post(charlie.url(&api::prefetch_path(song.id)))
        .bearer_auth(&charlie.token)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    charlie.wait_until_local(song.id).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr, charlie.node.p2p_addr]).await;
    let query = format!(
        "{}?q=twilight&network=true&discovery=true",
        api::tracks_path()
    );
    let page: TrackPage = bob.json(&query).await;
    assert_eq!(page.total, 1);
    drop(alice);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let page: TrackPage = bob.json(&query).await;
    assert_eq!(
        page.total, 1,
        "an online second copy must keep the song discoverable"
    );
    drop(charlie);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let page: TrackPage = bob.json(&query).await;
    assert_eq!(page.total, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_variants_share_one_audio_and_count_distinct_copies() {
    use hopea_common::api::{MetadataVariant, PendingCover};
    let alice = TestNode::start("alice", vec![]).await;
    let bob = TestNode::start("bob", vec![alice.node.p2p_addr]).await;
    let shared: UploadResponse = alice
        .share("Variant%20Artist%20-%20Song.flac", fixture("tone.flac"))
        .await
        .json()
        .await
        .unwrap();
    let aid = shared.track.id;
    async fn cover(node: &TestNode, id: i64, color: u8) -> String {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(32, 32, image::Rgb([color, 32, 128]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let pending: PendingCover = node
            .http
            .post(node.url(&api::covers_path()))
            .bearer_auth(&node.token)
            .body(png.into_inner())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let r = node
            .http
            .put(node.url(&api::cover_path(id)))
            .bearer_auth(&node.token)
            .json(&pending)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        pending.cover
    }
    async fn lyrics(node: &TestNode, id: i64, text: &str) {
        assert_eq!(
            node.http
                .put(node.url(&format!("{}/lyrics", api::track_path(id))))
                .bearer_auth(&node.token)
                .body(text.to_string())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    async fn variants(node: &TestNode, id: i64, refresh: bool) -> Vec<MetadataVariant> {
        node.json(&format!(
            "{}/variants?refresh={refresh}",
            api::track_path(id)
        ))
        .await
    }
    async fn select(node: &TestNode, id: i64, hash: &str) {
        assert_eq!(
            node.http
                .put(node.url(&format!("{}/variants/{hash}", api::track_path(id))))
                .bearer_auth(&node.token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    cover(&alice, aid, 180).await;
    cover(&alice, aid, 230).await;
    cover(&alice, aid, 230).await;
    lyrics(&alice, aid, "[00:00.00]Original words").await;
    lyrics(&alice, aid, "[00:00.00]Alternate words").await;
    lyrics(&alice, aid, "[00:00.00]Alternate words").await;
    let av = variants(&alice, aid, false).await;
    assert_eq!(
        av.len(),
        4,
        "repeated artwork/lyrics must reuse the same variants"
    );
    assert_eq!(av.iter().filter(|v| v.selected).count(), 2);
    let bid = bob.search("variant", true).await.tracks[0].id;
    bob.get(&api::audio_path(bid))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    bob.wait_until_local(bid).await;
    let bv = variants(&bob, bid, true).await;
    assert_eq!(bv.len(), 4);
    for v in &av {
        let copy = bv.iter().find(|b| b.hash == v.hash).unwrap();
        assert_eq!(copy.peers, 1, "Alice shares each contributed variant once");
        if v.selected {
            select(&bob, bid, &v.hash).await;
        }
    }
    let charlie = TestNode::start("charlie", vec![alice.node.p2p_addr, bob.node.p2p_addr]).await;
    let page = charlie.search("variant", true).await;
    assert_eq!(page.total, 1);
    let cid = page.tracks[0].id;
    let cv = variants(&charlie, cid, true).await;
    assert_eq!(cv.len(), 4);
    for v in &cv {
        let selected = av.iter().find(|a| a.hash == v.hash).unwrap().selected;
        assert_eq!(
            v.peers,
            if selected { 2 } else { 1 },
            "Bob only shares chosen variants; Alice shares contributed alternatives"
        );
    }
    let cv_again = variants(&charlie, cid, true).await;
    assert_eq!(
        cv.iter().map(|v| v.peers).collect::<Vec<_>>(),
        cv_again.iter().map(|v| v.peers).collect::<Vec<_>>()
    );
    // A listener's alternative doesn't replace the original publisher's lyrics.
    lyrics(&bob, bid, "[00:00.00]My translation").await;
    assert_eq!(
        bob.json::<Option<String>>(&format!("{}/lyrics", api::track_path(bid)))
            .await
            .as_deref(),
        Some("[00:00.00]My translation")
    );
    assert_eq!(
        alice
            .json::<Option<String>>(&format!("{}/lyrics", api::track_path(aid)))
            .await
            .as_deref(),
        Some("[00:00.00]Alternate words")
    );
    let reupload: UploadResponse = bob
        .share("Another%20Name.flac", fixture("tone.flac"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        reupload.track.id, bid,
        "a local downloaded copy also prevents a new audio entry"
    );
    assert_eq!(reupload.track.sha256, shared.track.sha256);
    assert_eq!(
        bob.json::<TrackPage>(&format!("{}?local=true", api::tracks_path()))
            .await
            .total,
        1
    );
    assert_eq!(
        alice
            .json::<TrackPage>(&format!("{}?local=true", api::tracks_path()))
            .await
            .total,
        1
    );
}
