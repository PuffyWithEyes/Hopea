// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A private, cancellable Spotify worker; stdin/stdout JSON only.
use hopea_common::service_import::CatalogTrack;
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::{authentication::Credentials, config::SessionConfig, Session, SpotifyUri};
use librespot_metadata::audio::{AudioFileFormat, AudioItem};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    time::Duration,
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
const MAX_AUDIO: u64 = 512 * 1024 * 1024;
const MAX_MESSAGE: u64 = 256 * 1024;
#[derive(Deserialize)]
struct Start {
    secret: String,
    directory: PathBuf,
}
fn audio_path(directory: &Path, key: &str) -> PathBuf {
    directory.join(format!(
        "{}.flac",
        hex::encode(Sha256::digest(format!("spotify:{key}")))
    ))
}
fn emit(value: serde_json::Value) -> Result<(), String> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|_| "Unable to send import result")?;
    output
        .write_all(b"\n")
        .and_then(|_| output.flush())
        .map_err(|_| "Unable to send import result".into())
}

async fn read_message<T: for<'a> Deserialize<'a>>(
    input: &mut (impl AsyncBufRead + Unpin),
) -> Result<Option<T>, String> {
    let mut bytes = Vec::new();
    let n = input
        .take(MAX_MESSAGE + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| "Unable to read Spotify import request")?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_MESSAGE as usize || bytes.last() != Some(&b'\n') {
        return Err("Invalid Spotify import request".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Invalid Spotify import request".into())
}

/// Does not initialize any logger: library trace logs can contain tokens.
fn main() -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    let result = runtime.block_on(async {
        #[cfg(unix)]
        {
            let mut stop =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|_| "Unable to initialize Spotify cancellation".to_string())?;
            tokio::select! { result = worker() => result, _ = stop.recv() => Ok(()) }
        }
        #[cfg(not(unix))]
        worker().await
    });
    let success = result.is_ok();
    if let Err(message) = result {
        let _ = emit(serde_json::json!({"event": "error", "message": message}));
    }
    // A cancelled blocking download must not hold the application open.
    runtime.shutdown_timeout(Duration::from_secs(1));
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn worker() -> Result<(), String> {
    let mut input = tokio::io::BufReader::new(tokio::io::stdin());
    let Some(mut start): Option<Start> = read_message(&mut input).await? else {
        return Ok(());
    };
    if start.secret.trim().is_empty()
        || start.secret.len() > 16384
        || !start.directory.is_absolute()
    {
        return Err("A Spotify access token and an absolute import directory are required".into());
    }
    // Defer all network requests until the first selected, uncached track.
    let mut session = None;
    while let Some(track) = read_message::<CatalogTrack>(&mut input).await? {
        // Spotify playlists can include the user's local files. They have
        // metadata but no downloadable Spotify stream; keep them as names.
        if track.key.starts_with("spotify:local:") {
            emit(serde_json::json!({"event": "downloaded", "key": track.key, "path": null}))?;
            continue;
        }
        let result = tokio::time::timeout(Duration::from_secs(600), async {
            let uri = track_uri(&track.key)?;
            let path = audio_path(&start.directory, &track.key);
            if cached_audio(&path)? { return Ok(Some(path)); }
            std::fs::create_dir_all(&start.directory).map_err(|_| "Unable to create Spotify import directory")?;
            if session.is_none() {
                for program in ["ffmpeg", "ffprobe"] {
                    let ready = tokio::process::Command::new(program).arg("-version")
                        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                        .kill_on_drop(true).status().await;
                    if !ready.is_ok_and(|status| status.success()) {
                        return Err("Spotify audio import requires FFmpeg and FFprobe. Install them before retrying.".to_string());
                    }
                }
                let next = Session::new(SessionConfig::default(), None);
                tokio::time::timeout(Duration::from_secs(45), next.connect(Credentials::with_access_token(&start.secret), false)).await
                    .map_err(|_| "Spotify sign-in timed out; check your connection")?
                    .map_err(|_| "Spotify rejected the audio session. Use a fresh token with streaming permission and a Premium account.")?;
                start.secret.clear();
                session = Some(next);
            }
            download(session.as_ref().unwrap(), uri, &track, &path).await
        }).await.map_err(|_| "Spotify download timed out. Completed songs were kept.")??;
        emit(serde_json::json!({"event": "downloaded", "key": track.key, "path": result}))?;
    }
    if let Some(session) = session {
        session.shutdown();
    }
    Ok(())
}

fn track_uri(key: &str) -> Result<SpotifyUri, String> {
    // The catalogue supplies base62 track IDs, not arbitrary URLs or local paths.
    if key.len() != 22 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Spotify returned an invalid track ID".into());
    }
    SpotifyUri::from_uri(&format!("spotify:track:{key}"))
        .map_err(|_| "Spotify returned an invalid track ID".into())
}

fn cached_audio(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Ok(info) if info.is_file() && info.len() > 4 && info.len() <= MAX_AUDIO => {
            let mut magic = [0; 4];
            std::fs::File::open(path)
                .and_then(|mut f| f.read_exact(&mut magic))
                .map_err(|_| "Unable to read cached Spotify audio")?;
            if &magic == b"fLaC" {
                Ok(true)
            } else {
                Err(
                    "Cached Spotify audio is invalid; remove that cached file before retrying"
                        .into(),
                )
            }
        }
        _ => Err("Unable to safely reuse cached Spotify audio".into()),
    }
}

async fn download(
    session: &Session,
    uri: SpotifyUri,
    track: &CatalogTrack,
    path: &Path,
) -> Result<Option<PathBuf>, String> {
    if session
        .get_user_attribute("type")
        .is_some_and(|kind| kind != "premium")
    {
        return Err("Direct Spotify audio import requires Spotify Premium".into());
    }
    let item = AudioItem::get_file(session, uri)
        .await
        .map_err(|_| "Unable to read Spotify audio metadata")?;
    if item.duration_ms == 0 || item.duration_ms > 3 * 60 * 60 * 1000 {
        return Err("Spotify audio exceeds the duration limit".into());
    }
    if item.availability.is_err() {
        return Ok(None);
    }
    let Some((format, file_id)) = [
        AudioFileFormat::OGG_VORBIS_320,
        AudioFileFormat::OGG_VORBIS_160,
        AudioFileFormat::OGG_VORBIS_96,
    ]
    .iter()
    .find_map(|format| item.files.get(format).map(|id| (*format, *id))) else {
        return Ok(None);
    };
    let SpotifyUri::Track { id } = item.track_id else {
        return Err("Spotify did not return a music track".into());
    };
    let key = session.audio_key().request(id, file_id).await
        .map_err(|_| "Spotify refused the audio key. Some accounts are incompatible with librespot after Spotify changes; this importer cannot override that restriction. Try importing song names only.")?;
    let rate = match format {
        AudioFileFormat::OGG_VORBIS_320 => 40000,
        AudioFileFormat::OGG_VORBIS_160 => 20000,
        _ => 12000,
    };
    let file = AudioFile::open(session, file_id, rate)
        .await
        .map_err(|_| "Unable to open Spotify audio")?;
    let control = file
        .get_stream_loader_controller()
        .map_err(|_| "Unable to inspect Spotify audio")?;
    if control.len() as u64 > MAX_AUDIO {
        return Err("Spotify audio exceeds the file size limit".into());
    }
    let source = tokio::task::spawn_blocking(move || -> Result<tempfile::NamedTempFile, String> {
        let mut reader = AudioDecrypt::new(Some(key), file);
        // Spotify's extra Ogg header is omitted by librespot's player too.
        reader
            .seek(SeekFrom::Start(0xa7))
            .map_err(|_| "Unable to read Spotify audio")?;
        let mut signature = [0; 4];
        reader
            .read_exact(&mut signature)
            .map_err(|_| "Incomplete Spotify audio")?;
        if &signature != b"OggS" {
            return Err("Spotify returned an unsupported audio stream".into());
        }
        let mut source =
            tempfile::NamedTempFile::new().map_err(|_| "Unable to stage Spotify audio")?;
        source
            .write_all(&signature)
            .map_err(|_| "Unable to stage Spotify audio")?;
        let n = std::io::copy(&mut reader.take(MAX_AUDIO + 1), &mut source)
            .map_err(|_| "Spotify audio download failed")?;
        if n + 4 > MAX_AUDIO {
            return Err("Spotify audio exceeds the file size limit".into());
        }
        source
            .flush()
            .map_err(|_| "Unable to stage Spotify audio")?;
        control.close();
        Ok(source)
    })
    .await
    .map_err(|_| "Spotify audio download failed")??;
    convert(source.path(), path, track, item.duration_ms).await?;
    Ok(Some(path.into()))
}

async fn convert(
    source: &Path,
    path: &Path,
    track: &CatalogTrack,
    duration_ms: u32,
) -> Result<(), String> {
    let output = tempfile::NamedTempFile::new_in(path.parent().unwrap())
        .map_err(|_| "Unable to stage Spotify audio")?;
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-xerror",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "ogg",
            "-i",
        ])
        .arg(source)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-map_metadata",
            "-1",
            "-c:a",
            "flac",
            "-fs",
            "536870912",
            "-t",
            "10801",
        ]);
    for (name, value) in [
        ("title", Some(track.title.as_str())),
        ("artist", track.artist.as_deref()),
        ("album", track.album.as_deref()),
        ("album_artist", track.album_artist.as_deref()),
    ] {
        if let Some(value) = value {
            command.arg("-metadata").arg(format!("{name}={value}"));
        }
    }
    if let Some(number) = track.track_number {
        command.arg("-metadata").arg(format!("track={number}"));
    }
    let status = command
        .args(["-f", "flac"])
        .arg(output.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Unable to start FFmpeg; install it before importing Spotify audio")?
        .wait()
        .await
        .map_err(|_| "Unable to convert Spotify audio")?;
    if !status.success() || !cached_audio(output.path())? {
        return Err("Unable to convert Spotify audio".into());
    }
    // Check completion before publishing a checkpoint. The node performs its
    // full decode/sanitization before this file can be added to sharing.
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(output.path())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| "Unable to check Spotify audio; install FFprobe with FFmpeg")?;
    if !probe.status.success() || !duration_matches(&probe.stdout, duration_ms) {
        return Err("Spotify returned incomplete audio; it was not imported".into());
    }
    output
        .persist_noclobber(path)
        .map_err(|_| "Unable to save Spotify audio without overwriting an existing file")?;
    Ok(())
}
fn duration_matches(json: &[u8], expected_ms: u32) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else {
        return false;
    };
    let actual = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());
    actual.is_some_and(|s| {
        s.is_finite() && s > 0.0 && (s * 1000.0 - f64::from(expected_ms)).abs() <= 2000.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> CatalogTrack {
        CatalogTrack {
            key: "4uLU6hMCjMI75M1A2tKUQC".into(),
            title: "Title; $(not-a-command)".into(),
            duration_ms: None,
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            album_artist: Some("Album Artist".into()),
            track_number: Some(3),
            cover: None,
            collections: vec![],
        }
    }

    #[tokio::test]
    async fn protocol_rejects_oversized_and_unterminated_input() {
        for data in [vec![b'x'; MAX_MESSAGE as usize + 1], b"{}".to_vec()] {
            assert!(read_message::<serde_json::Value>(&mut data.as_slice())
                .await
                .is_err());
        }
        assert!(read_message::<serde_json::Value>(&mut &b""[..])
            .await
            .unwrap()
            .is_none());
        assert!(read_message::<serde_json::Value>(&mut &b"{}\n"[..])
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn only_spotify_track_ids_are_accepted() {
        assert!(track_uri(&track().key).is_ok());
        for id in [
            "",
            "../../some-file",
            "spotify:local:test",
            "https://example.com/audio",
            "☃☃☃☃☃☃☃",
        ] {
            assert!(track_uri(id).is_err());
        }
    }

    #[test]
    fn incomplete_or_invalid_audio_is_not_a_successful_checkpoint() {
        assert!(duration_matches(
            br#"{"format":{"duration":"120.001"}}"#,
            120000
        ));
        for bytes in [
            br#"{"format":{"duration":"10.0"}}"#.as_slice(),
            br#"{"format":{"duration":"NaN"}}"#,
            br#"{}"#,
        ] {
            assert!(!duration_matches(bytes, 120000));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = audio_path(dir.path(), &track().key);
        assert!(!cached_audio(&path).unwrap());
        std::fs::write(&path, b"partial audio").unwrap();
        assert!(cached_audio(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_never_used_as_an_audio_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"fLaCdata").unwrap();
        let path = audio_path(dir.path(), &track().key);
        std::os::unix::fs::symlink(target, &path).unwrap();
        assert!(cached_audio(&path).is_err());
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg and FFprobe; run inside nix-shell with --include-ignored"]
    async fn real_conversion_keeps_tags_and_never_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../node/tests/fixtures/tone.ogg");
        let probe = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
            ])
            .arg(&source)
            .output()
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let duration = (value["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            * 1000.0)
            .round() as u32;
        let song = track();
        let path = audio_path(dir.path(), &song.key);
        convert(&source, &path, &song, duration).await.unwrap();
        let original = std::fs::read(&path).unwrap();
        let probe = tokio::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format_tags", "-of", "json"])
            .arg(&path)
            .output()
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["format"]["tags"]["title"], song.title);
        assert_eq!(value["format"]["tags"]["album"], "Album");
        assert_eq!(value["format"]["tags"]["track"], "3");
        assert!(convert(&source, &path, &song, duration).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let incomplete = dir.path().join("incomplete.flac");
        assert!(convert(&source, &incomplete, &song, duration + 10000)
            .await
            .is_err());
        assert!(!incomplete.exists());
    }
}
