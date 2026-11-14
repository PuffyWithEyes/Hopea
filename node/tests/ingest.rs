// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs real and deliberately hostile files through the upload pipeline.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use hopea_common::AudioFormat;
use hopea_node::{
    config::Limits,
    ingest::{ingest, Ingested, Rejection},
};
use lofty::{
    config::WriteOptions,
    picture::{MimeType, Picture, PictureType},
    prelude::{Accessor, TagExt, TaggedFileExt},
    tag::Tag,
};
use tempfile::TempDir;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(path).unwrap()
}

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.dir.path().join(format!("upload-{name}"));
        fs::write(&path, data).unwrap();
        path
    }

    fn ingest_as(&self, name: &str, data: &[u8]) -> Result<Ingested, Rejection> {
        self.ingest_with(name, data, &Limits::default())
    }

    fn ingest_with(&self, name: &str, data: &[u8], limits: &Limits) -> Result<Ingested, Rejection> {
        let input = self.write(name, data);
        ingest(&input, name, self.dir.path(), limits)
    }
}

/// Writes a tag with a cover picture into a copy of `data` using lofty, the
/// same way a desktop tagger would.
fn tagged(env: &Env, ext: &str, data: &[u8], picture: &[u8]) -> Vec<u8> {
    let path = env.write(&format!("tagged.{ext}"), data);
    let mut file = lofty::read_from_path(&path).unwrap_or_else(|e| panic!("{ext}: {e}"));
    let tag_type = file.primary_tag_type();
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.primary_tag_mut().unwrap();
    tag.set_title("Night\u{202E}Drive\u{0007}".into());
    tag.set_artist("The Testers".into());
    tag.set_album("Fixtures".into());
    tag.push_picture(
        Picture::unchecked(picture.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );
    tag.save_to_path(&path, WriteOptions::default()).unwrap();
    fs::read(&path).unwrap()
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Bytes that are easy to spot in the output if they survive sanitization.
fn payload(len: usize) -> Vec<u8> {
    let mut data = b"PK\x03\x04MALICIOUS-PAYLOAD".to_vec();
    let mut x: u32 = 0x1234_5678;
    while data.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        data.push((x >> 24) as u8);
    }
    data
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn output(ingested: &Ingested) -> Vec<u8> {
    fs::read(&ingested.file).unwrap()
}

#[test]
fn accepts_every_supported_format() {
    let env = Env::new();
    for (name, format, codec) in [
        ("tone.flac", AudioFormat::Flac, "FLAC"),
        ("tone.mp3", AudioFormat::Mp3, "MP3"),
        ("tone.ogg", AudioFormat::Ogg, "Vorbis"),
        ("tone.wav", AudioFormat::Wav, "PCM"),
        ("tone.m4a", AudioFormat::Mp4, "AAC"),
        ("tone-alac.m4a", AudioFormat::Mp4, "ALAC"),
    ] {
        let ingested = env
            .ingest_as(name, &fixture(name))
            .unwrap_or_else(|e| panic!("{name} rejected: {e}"));
        assert_eq!(ingested.format, format, "{name}");
        assert_eq!(ingested.codec, codec, "{name}");
        assert_eq!(ingested.sample_rate, 22050, "{name}");
        assert_eq!(ingested.channels, 1, "{name}");
        assert!(
            (900..1200).contains(&ingested.duration_ms),
            "{name}: {} ms",
            ingested.duration_ms
        );
        assert!(!ingested.peaks.is_empty(), "{name}");
        assert!(ingested.peaks.iter().any(|(l, _)| *l > 0.1), "{name}");
        let stem = name.rsplit_once('.').unwrap().0;
        assert_eq!(ingested.tags.title.as_deref(), Some(stem), "{name}");
        assert_eq!(ingested.sha256.len(), 64);
    }
}

#[test]
fn mp4_extension_is_refused_even_for_audio() {
    // Only audio formats are accepted, and .mp4 names video files
    let env = Env::new();
    let err = env.ingest_as("song.mp4", &fixture("tone.m4a")).unwrap_err();
    assert_eq!(err.code(), "unsupported_type", "{err}");
}

#[test]
fn rejects_unlisted_extensions_before_reading() {
    let env = Env::new();
    for name in ["setup.exe", "song.flac.exe", "song.opus", "song", "song."] {
        let err = env.ingest_as(name, &fixture("tone.flac")).unwrap_err();
        assert_eq!(err.code(), "unsupported_type", "{name}");
    }
}

#[test]
fn rejects_renamed_executable() {
    let env = Env::new();
    let mut exe = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xFF\xFF".to_vec();
    exe.extend(payload(4096));
    let err = env.ingest_as("song.flac", &exe).unwrap_err();
    assert!(
        matches!(err, Rejection::NotAudio(Some("a Windows executable"))),
        "{err}"
    );

    let mut elf = b"\x7fELF\x02\x01\x01".to_vec();
    elf.extend(payload(4096));
    let err = env.ingest_as("song.mp3", &elf).unwrap_err();
    assert_eq!(err.code(), "not_audio");
}

#[test]
fn rejects_html_disguised_as_audio() {
    let env = Env::new();
    let html = b"<!DOCTYPE html><script>alert(document.cookie)</script>".repeat(10);
    let err = env.ingest_as("song.wav", &html).unwrap_err();
    assert!(matches!(err, Rejection::NotAudio(Some(_))), "{err}");
}

#[test]
fn rejects_extension_that_does_not_match_content() {
    let env = Env::new();
    let err = env
        .ingest_as("song.flac", &fixture("tone.mp3"))
        .unwrap_err();
    assert!(
        matches!(err, Rejection::ExtensionMismatch { ref claimed, actual: "MP3" } if claimed == "flac"),
        "{err}"
    );
}

#[test]
fn rejects_magic_bytes_followed_by_garbage() {
    let env = Env::new();
    let mut fake = b"fLaC".to_vec();
    fake.extend(payload(8192));
    assert!(env.ingest_as("song.flac", &fake).is_err());

    let mut fake = b"RIFF\x00\x10\x00\x00WAVE".to_vec();
    fake.extend(payload(8192));
    assert!(env.ingest_as("song.wav", &fake).is_err());

    let mut fake = vec![0xFF, 0xFB, 0x90, 0x64];
    fake.extend(payload(8192));
    assert!(env.ingest_as("song.mp3", &fake).is_err());
}

#[test]
fn rejects_unsupported_audio() {
    let env = Env::new();
    let err = env
        .ingest_as("song.ogg", &fixture("tone.opus"))
        .unwrap_err();
    assert!(
        matches!(err, Rejection::NotAudio(Some(d)) if d.contains("Opus")),
        "{err}"
    );
}

#[test]
fn rejects_mpeg4_with_video_track() {
    let env = Env::new();
    // A video file renamed to look like audio
    let err = env
        .ingest_as("clip.m4a", &fixture("with-video.mp4"))
        .unwrap_err();
    assert_eq!(err.code(), "unsupported_audio", "{err}");
}

#[test]
fn rejects_truncated_files() {
    let env = Env::new();
    for name in ["tone.flac", "tone.wav", "tone.m4a"] {
        let data = fixture(name);
        let half = &data[..data.len() / 2];
        assert!(env.ingest_as(name, half).is_err(), "{name}");
    }
}

#[test]
fn rejects_corrupted_flac_audio() {
    let env = Env::new();
    let mut data = fixture("tone.flac");
    let n = data.len();
    for b in &mut data[n / 2..n / 2 + 64] {
        *b ^= 0x5A;
    }
    assert!(env.ingest_as("tone.flac", &data).is_err());
}

#[test]
fn drops_data_appended_after_the_audio() {
    let env = Env::new();
    let secret = payload(300 * 1024);
    for name in ["tone.flac", "tone.wav", "tone.mp3"] {
        let mut data = fixture(name);
        data.extend_from_slice(&secret);
        match env.ingest_as(name, &data) {
            // Either the rebuild drops the payload, or the file is rejected
            Ok(ingested) => assert!(
                !contains(&output(&ingested), b"MALICIOUS-PAYLOAD"),
                "{name}: payload survived"
            ),
            Err(err) => assert!(
                matches!(err.code(), "hidden_data" | "malformed"),
                "{name}: {err}"
            ),
        }
    }
}

#[test]
fn rejects_data_hidden_between_mp3_frames() {
    let env = Env::new();
    let data = fixture("tone.mp3");
    let mid = data.len() / 2;
    // Find a frame boundary near the middle
    let at = (mid..data.len() - 1)
        .find(|&i| data[i] == 0xFF && data[i + 1] & 0xE0 == 0xE0)
        .unwrap();
    let mut spliced = data[..at].to_vec();
    spliced.extend(payload(300 * 1024));
    spliced.extend_from_slice(&data[at..]);
    let err = env.ingest_as("tone.mp3", &spliced).unwrap_err();
    assert!(matches!(err.code(), "hidden_data" | "malformed"), "{err}");
}

#[test]
fn rejects_trailing_ogg_garbage_or_drops_it() {
    let env = Env::new();
    let mut data = fixture("tone.ogg");
    data.extend(payload(300 * 1024));
    if let Ok(ingested) = env.ingest_as("tone.ogg", &data) {
        assert!(!contains(&output(&ingested), b"MALICIOUS-PAYLOAD"));
    }
}

/// Inserts an APPLICATION metadata block carrying `body` after STREAMINFO.
fn flac_with_application_block(body: &[u8]) -> Vec<u8> {
    let data = fixture("tone.flac");
    let streaminfo_end = 4 + 4 + 34;
    let mut out = data[..streaminfo_end].to_vec();
    out[4] &= 0x7F; // STREAMINFO is no longer the last block
    let len = (body.len() as u32 + 4).to_be_bytes();
    out.push(2);
    out.extend_from_slice(&len[1..4]);
    out.extend_from_slice(b"evil");
    out.extend_from_slice(body);
    out.extend_from_slice(&data[streaminfo_end..]);
    out
}

#[test]
fn strips_flac_application_blocks() {
    let env = Env::new();
    let data = flac_with_application_block(&payload(200 * 1024));
    let ingested = env.ingest_as("tone.flac", &data).unwrap();
    let out = output(&ingested);
    assert!(!contains(&out, b"MALICIOUS-PAYLOAD"));
    assert!(out.len() < fixture("tone.flac").len() + 1024);
}

#[test]
fn strips_wav_extra_chunks() {
    let env = Env::new();
    let data = fixture("tone.wav");
    let secret = payload(64 * 1024);
    let mut out = data[..12].to_vec();
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&(secret.len() as u32).to_le_bytes());
    out.extend_from_slice(&secret);
    out.extend_from_slice(&data[12..]);
    let riff_len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_len.to_le_bytes());

    let ingested = env.ingest_as("tone.wav", &out).unwrap();
    let cleaned = output(&ingested);
    assert!(!contains(&cleaned, b"MALICIOUS-PAYLOAD"));
    assert!(cleaned.len() <= data.len());
}

#[test]
fn strips_mp3_trailing_tags() {
    let env = Env::new();
    let mut data = fixture("tone.mp3");
    // APEv2 tag with a footer only
    let items = payload(2048);
    let mut ape = items.clone();
    ape.extend_from_slice(b"APETAGEX");
    ape.extend_from_slice(&2000u32.to_le_bytes());
    ape.extend_from_slice(&((items.len() + 32) as u32).to_le_bytes());
    ape.extend_from_slice(&1u32.to_le_bytes());
    ape.extend_from_slice(&0u32.to_le_bytes());
    ape.extend_from_slice(&[0; 8]);
    data.extend_from_slice(&ape);
    // ID3v1
    let mut id3v1 = b"TAG".to_vec();
    id3v1.resize(128, b'x');
    data.extend_from_slice(&id3v1);

    let ingested = env.ingest_as("tone.mp3", &data).unwrap();
    assert!(!contains(&output(&ingested), b"MALICIOUS-PAYLOAD"));
}

#[test]
fn rewrites_tags_and_reencodes_cover_art() {
    let env = Env::new();
    let cover = png(1500, 1500);
    for name in ["tone.flac", "tone.mp3", "tone.ogg", "tone.m4a"] {
        let ext = name.rsplit_once('.').unwrap().1;
        let data = tagged(&env, ext, &fixture(name), &cover);
        let ingested = env
            .ingest_as(name, &data)
            .unwrap_or_else(|e| panic!("{name} rejected: {e}"));

        assert_eq!(ingested.tags.title.as_deref(), Some("NightDrive"), "{name}");
        assert_eq!(
            ingested.tags.artist.as_deref(),
            Some("The Testers"),
            "{name}"
        );
        assert_eq!(ingested.tags.album.as_deref(), Some("Fixtures"), "{name}");

        let jpeg = ingested
            .cover_jpeg
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: no cover"));
        assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]), "{name}");
        let decoded = image::load_from_memory(jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 1024), "{name}");

        // The original PNG must not survive in the stored file
        let out = output(&ingested);
        assert!(!contains(&out, &cover[..64]), "{name}: picture survived");

        // Tags written back (where the format carries them) are readable
        if ext != "m4a" {
            let reread = lofty::probe::Probe::open(&ingested.file)
                .unwrap()
                .guess_file_type()
                .unwrap()
                .read()
                .unwrap();
            let tag = reread.primary_tag().unwrap();
            assert_eq!(tag.title().as_deref(), Some("NightDrive"), "{name}");
            assert!(tag.pictures().is_empty(), "{name}");
        }
    }
}

#[test]
fn discards_cover_art_that_is_not_an_image() {
    let env = Env::new();
    let data = tagged(&env, "flac", &fixture("tone.flac"), &payload(32 * 1024));
    let ingested = env.ingest_as("tone.flac", &data).unwrap();
    assert!(ingested.cover_jpeg.is_none());
    assert!(!contains(&output(&ingested), b"MALICIOUS-PAYLOAD"));
}

#[test]
fn compressed_id3_payloads_are_not_expanded() {
    // zlib payload for an ID3 Latin-1 title containing 10,000 x characters.
    // Keep the regression fixture small even if compression is re-enabled.
    let compressed =
        hex::decode("789cedc1010900000002a06637bf1fa1a600000000000000000070600102508f").unwrap();
    let frame_size = compressed.len() as u32 + 4;
    let tag_size = frame_size + 10;
    let mut data = b"ID3\x03\0\0".to_vec();
    data.extend([
        (tag_size >> 21) as u8 & 0x7f,
        (tag_size >> 14) as u8 & 0x7f,
        (tag_size >> 7) as u8 & 0x7f,
        tag_size as u8 & 0x7f,
    ]);
    data.extend(b"TIT2");
    data.extend(frame_size.to_be_bytes());
    data.extend([0, 0x80]); // ID3v2.3 compression flag
    data.extend(10001u32.to_be_bytes());
    data.extend(compressed);
    data.extend(fixture("tone.mp3"));
    let env = Env::new();
    let ingested = env.ingest_as("bomb.mp3", &data).unwrap();
    assert_eq!(ingested.tags.title.as_deref(), Some("bomb"));
    assert!(ingested.cover_jpeg.is_none());
}

#[test]
fn enforces_duration_limit() {
    let env = Env::new();
    let limits = Limits {
        max_duration: Duration::ZERO,
        ..Limits::default()
    };
    let err = env
        .ingest_with("tone.flac", &fixture("tone.flac"), &limits)
        .unwrap_err();
    assert_eq!(err.code(), "too_long");
}

#[test]
fn enforces_size_limit() {
    let env = Env::new();
    let limits = Limits {
        max_upload_bytes: 1024,
        ..Limits::default()
    };
    let err = env
        .ingest_with("tone.wav", &fixture("tone.wav"), &limits)
        .unwrap_err();
    assert_eq!(err.code(), "too_large");
}

#[test]
fn identical_audio_hashes_identically() {
    let env = Env::new();
    let plain = env.ingest_as("a.flac", &fixture("tone.flac")).unwrap();
    let with_junk = env
        .ingest_as("a.flac", &flac_with_application_block(&payload(1024)))
        .unwrap();
    assert_eq!(plain.sha256, with_junk.sha256);
}

#[test]
fn legitimate_files_have_little_overhead() {
    let env = Env::new();
    for name in [
        "tone.flac",
        "tone.mp3",
        "tone.ogg",
        "tone.wav",
        "tone.m4a",
        "tone-alac.m4a",
    ] {
        let ingested = env.ingest_as(name, &fixture(name)).unwrap();
        println!(
            "{name}: {} bytes, {} non-audio, {} allowed",
            ingested.size, ingested.overhead_bytes, ingested.overhead_allowed
        );
        assert!(ingested.overhead_bytes < 8 * 1024, "{name}");
    }
}
