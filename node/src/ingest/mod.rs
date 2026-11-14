// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Upload validation and sanitization.
//!
//! Nothing about an upload is trusted. A file is only accepted once it has
//! passed every one of these stages:
//!
//! 1. **Name**: the extension must be on the allowlist in
//!    [`AudioFormat`]. The rest of the name is only used as a fallback title.
//! 2. **Signature**: the first bytes must be the container signature of
//!    exactly the format the extension claims. A renamed executable, archive
//!    or document stops here.
//! 3. **Rebuild**: the file is rewritten into a fresh file that keeps only
//!    the audio stream and the headers needed to decode it. All tags,
//!    embedded pictures, padding, application blocks, unknown chunks and
//!    trailing data are dropped (or, for MP4, overwritten with zeros). Tags
//!    are re-created from sanitized text, and the cover art is decoded and
//!    re-encoded as a plain JPEG.
//! 4. **Decode**: the *rebuilt* file is demuxed and every packet decoded
//!    with Symphonia (pure Rust, `#![forbid(unsafe_code)]`). It must contain
//!    a single audio stream in an allowed codec, decode without errors,
//!    stay within the duration, channel and sample rate limits, and pass the
//!    codec's own checksums where it has them.
//! 5. **Accounting**: almost every byte of the rebuilt file must belong to a
//!    decoded audio packet. Anything else, such as junk between MP3 frames
//!    or extra Ogg streams, counts against a small budget for container
//!    overhead. This is what catches polyglot files and data smuggled
//!    inside an otherwise valid file.
//!
//! Only the rebuilt file is ever stored or shared, so the bytes other nodes
//! download are exactly the bytes that were decoded in stage 4. A node that
//! downloads a track runs the same pipeline on it ([`verify_download`]) and
//! keeps it only if the rebuild comes out byte for byte identical: that is,
//! only if the file is already exactly what validation produces.

mod analyze;
mod cover;
pub use self::cover::reencode as reencode_cover;
mod flac;
mod mp3;
mod mp4;
mod ogg;
mod tags;
mod wav;

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    time::Instant,
};

use hopea_common::{api::Peaks, describe_non_audio, AudioFormat, SNIFF_LEN};
use sha2::{Digest, Sha256};
use tempfile::TempPath;

pub use self::tags::Tags;
pub(crate) use self::{
    analyze::open_reader,
    tags::{sanitize, sanitize_short},
};
use crate::config::Limits;

/// Bytes of container overhead allowed regardless of file size: headers,
/// codec setup data, seek tables, our own tags.
const BASE_OVERHEAD: u64 = 64 * 1024;
/// Bytes of framing/index overhead allowed per audio packet: MP4 sample
/// tables, Ogg lacing values, FLAC and MP3 frame headers.
const PER_PACKET_OVERHEAD: u64 = 24;
/// Extra proportional allowance (1/200 = 0.5%)
const PROPORTIONAL_OVERHEAD_DIVISOR: u64 = 200;

/// A validated, sanitized upload ready to be stored.
#[derive(Debug)]
pub struct Ingested {
    pub format: AudioFormat,
    pub codec: &'static str,
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_ms: u64,
    pub size: u64,
    pub sha256: String,
    /// Identifies the audio regardless of tags; used to detect duplicates
    pub audio_sha256: String,
    pub tags: Tags,
    /// Re-encoded JPEG cover art
    pub cover_jpeg: Option<Vec<u8>>,
    pub peaks: Peaks,
    /// Bytes of the stored file that are not audio packets, and the budget
    pub overhead_bytes: u64,
    pub overhead_allowed: u64,
    /// The sanitized file; deleted on drop unless persisted
    pub file: TempPath,
}

#[derive(Debug, thiserror::Error)]
pub enum Rejection {
    #[error("The file name has no extension")]
    MissingExtension,
    #[error("Files ending in “.{0}” are not accepted. Accepted types: {list}", list = accepted_list())]
    UnsupportedExtension(String),
    #[error("The file is too small to contain audio")]
    TooSmall,
    #[error("The file is larger than the {0} MiB limit")]
    TooLarge(u64),
    #[error("This is not an audio file{}", .0.map(|d| format!("; it looks like {d}")).unwrap_or_default())]
    NotAudio(Option<&'static str>),
    #[error("The file is named “.{claimed}” but contains {actual}")]
    ExtensionMismatch {
        claimed: String,
        actual: &'static str,
    },
    #[error("The file is damaged or malformed: {0}")]
    Malformed(String),
    #[error("Unsupported audio: {0}")]
    Unsupported(String),
    #[error("The file contains {extra} bytes that are not audio, at most {allowed} are allowed")]
    HiddenData { extra: u64, allowed: u64 },
    #[error("The audio is longer than the {0} minute limit")]
    TooLong(u64),
    #[error("Checking the file took too long")]
    Timeout,
    #[error("Internal error while checking the file")]
    Internal(#[source] io::Error),
}

impl Rejection {
    /// Stable code for the API.
    pub fn code(&self) -> &'static str {
        match self {
            Rejection::MissingExtension | Rejection::UnsupportedExtension(_) => "unsupported_type",
            Rejection::TooSmall => "too_small",
            Rejection::TooLarge(_) => "too_large",
            Rejection::NotAudio(_) => "not_audio",
            Rejection::ExtensionMismatch { .. } => "extension_mismatch",
            Rejection::Malformed(_) => "malformed",
            Rejection::Unsupported(_) => "unsupported_audio",
            Rejection::HiddenData { .. } => "hidden_data",
            Rejection::TooLong(_) => "too_long",
            Rejection::Timeout => "timeout",
            Rejection::Internal(_) => "internal",
        }
    }
}

impl From<io::Error> for Rejection {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::UnexpectedEof => Rejection::Malformed("the file is truncated".into()),
            _ => Rejection::Internal(err),
        }
    }
}

fn accepted_list() -> String {
    AudioFormat::all_extensions()
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validates the upload at `input` and writes a sanitized copy to a new
/// temporary file in `work_dir`.
///
/// `file_name` is the name the uploader gave the file; this function is
/// blocking and CPU heavy, and must not run on an async executor thread.
pub fn ingest(
    input: &Path,
    file_name: &str,
    work_dir: &Path,
    limits: &Limits,
) -> Result<Ingested, Rejection> {
    let deadline = Instant::now() + limits.max_validation_time;

    // Stage 1: name
    let (stem, ext) = file_name
        .rsplit_once('.')
        .filter(|(_, ext)| !ext.is_empty())
        .ok_or(Rejection::MissingExtension)?;
    let format = AudioFormat::from_extension(ext)
        .ok_or_else(|| Rejection::UnsupportedExtension(tags::sanitize_short(ext, 16)))?;

    let input_file = File::open(input)?;
    let input_len = input_file.metadata()?.len();
    if input_len > limits.max_upload_bytes {
        return Err(Rejection::TooLarge(limits.max_upload_bytes / (1024 * 1024)));
    }
    if input_len < 64 {
        return Err(Rejection::TooSmall);
    }

    // Stage 2: signature
    let mut header = Vec::with_capacity(SNIFF_LEN);
    (&input_file)
        .take(SNIFF_LEN as u64)
        .read_to_end(&mut header)?;
    match AudioFormat::sniff(&header) {
        Some(actual) if actual == format => {}
        Some(actual) => {
            return Err(Rejection::ExtensionMismatch {
                claimed: ext.to_ascii_lowercase(),
                actual: actual.display_name(),
            })
        }
        None => return Err(Rejection::NotAudio(describe_non_audio(&header))),
    }

    // Tags and cover art are read from the original, then written back in
    // sanitized form only
    let (mut tags, raw_cover) = tags::read(input, format);
    if tags.title.is_none() {
        tags.title = Some(tags::fallback_title(stem));
    }
    let cover_jpeg = raw_cover.and_then(|data| cover::reencode(&data));

    // Stage 3: rebuild
    let output = tempfile::Builder::new()
        .prefix("sanitized-")
        .tempfile_in(work_dir)?;
    let (mut output_file, output_path) = output.into_parts();
    let rebuilt = match format {
        AudioFormat::Flac => flac::rebuild(&input_file, input_len, &mut output_file, &tags)?,
        AudioFormat::Mp3 => mp3::rebuild(&input_file, input_len, &mut output_file, &tags)?,
        AudioFormat::Wav => wav::rebuild(&input_file, input_len, &mut output_file)?,
        AudioFormat::Mp4 => {
            copy_range(&input_file, 0, input_len, &mut output_file)?;
            mp4::sanitize(&output_file)?
        }
        AudioFormat::Ogg => {
            copy_range(&input_file, 0, input_len, &mut output_file)?;
            drop(output_file);
            let rebuilt = ogg::retag(&output_path, &tags)?;
            output_file = File::options().read(true).write(true).open(&output_path)?;
            rebuilt
        }
    };
    output_file.sync_all()?;
    let size = output_file.metadata()?.len();
    drop(output_file);

    // Stage 4: decode the rebuilt file
    let analysis = analyze::analyze(&output_path, format, limits, deadline)?;

    // Stage 5: account for every byte
    let accounted = analysis.audio_bytes + rebuilt.inert_bytes + rebuilt.metadata_bytes;
    let extra = size.saturating_sub(accounted);
    let allowed = BASE_OVERHEAD
        + analysis.packets * PER_PACKET_OVERHEAD
        + size / PROPORTIONAL_OVERHEAD_DIVISOR;
    if extra > allowed {
        return Err(Rejection::HiddenData { extra, allowed });
    }

    let sha256 = hash_file(&output_path)?;

    Ok(Ingested {
        format,
        codec: analysis.codec,
        sample_rate: analysis.sample_rate,
        channels: analysis.channels,
        duration_ms: analysis.duration_ms,
        size,
        sha256,
        audio_sha256: analysis.audio_sha256,
        tags,
        cover_jpeg,
        peaks: analysis.peaks,
        overhead_bytes: extra,
        overhead_allowed: allowed,
        file: output_path,
    })
}

/// What a format specific rebuild step wrote.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Rebuilt {
    /// Bytes of metadata written by us (tags, required headers)
    pub metadata_bytes: u64,
    /// Bytes overwritten with zeros in place
    pub inert_bytes: u64,
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Reads exactly `len` bytes at `offset`, mapping short reads to a
/// truncation error.
pub(crate) fn read_at(file: &File, offset: u64, len: usize) -> Result<Vec<u8>, Rejection> {
    let mut buf = vec![0; len];
    crate::fsx::read_exact_at(file, &mut buf, offset)?;
    Ok(buf)
}

/// Copies `len` bytes starting at `offset` from `input` to `output`.
pub(crate) fn copy_range(
    input: &File,
    offset: u64,
    len: u64,
    output: &mut File,
) -> Result<(), Rejection> {
    let mut reader = io::BufReader::new(input);
    io::Seek::seek(&mut reader, io::SeekFrom::Start(offset))?;
    let copied = io::copy(&mut reader.take(len), output)?;
    if copied != len {
        return Err(Rejection::Malformed("the file is truncated".into()));
    }
    Ok(())
}

pub(crate) fn malformed(msg: impl Into<String>) -> Rejection {
    Rejection::Malformed(msg.into())
}

/// Checks a file downloaded from another node. It must pass every stage of
/// [`ingest`], and rebuilding it must produce the very same bytes, so that
/// nothing the rebuild would have dropped (tags, padding, appended data) can
/// travel from node to node.
pub fn verify_download(
    path: &Path,
    format: AudioFormat,
    expected_sha256: &str,
    expected_audio_sha256: &str,
    expected_duration_ms: u64,
    work_dir: &Path,
    limits: &Limits,
) -> Result<Ingested, Rejection> {
    let name = crate::torrent_info::file_name(format);
    let ingested = ingest(path, &name, work_dir, limits)?;
    let original = hash_file(path)?;
    if original != expected_sha256 || ingested.sha256 != original {
        return Err(malformed("the file is not in the form validation produces"));
    }
    if ingested.audio_sha256 != expected_audio_sha256 {
        return Err(malformed("the audio does not match its description"));
    }
    if ingested.duration_ms != expected_duration_ms {
        return Err(malformed(
            "the decoded duration does not match its description",
        ));
    }
    Ok(ingested)
}
