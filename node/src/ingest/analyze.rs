// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Demuxes and decodes every packet of a (sanitized) file.

use std::{fs::File, io, path::Path, time::Instant};

use hopea_common::{api::Peaks, AudioFormat};
use sha2::{Digest, Sha256};
use symphonia::{
    core::{
        codecs::{
            audio::{
                well_known::{
                    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_PCM_F32LE,
                    CODEC_ID_PCM_F64LE, CODEC_ID_PCM_S16LE, CODEC_ID_PCM_S24LE, CODEC_ID_PCM_S32LE,
                    CODEC_ID_PCM_U8, CODEC_ID_VORBIS,
                },
                AudioCodecId, AudioDecoderOptions,
            },
            CodecParameters,
        },
        errors::Error,
        formats::{FormatOptions, FormatReader},
        io::MediaSourceStream,
    },
    default::formats::{FlacReader, IsoMp4Reader, MpaReader, OggReader, WavReader},
};

use super::{malformed, Rejection};
use crate::config::Limits;

/// Waveform resolution expected by the client: one peak pair per 250 ms
const PEAKS_PER_SECOND: u32 = 4;
/// Tolerated recoverable decode errors, on top of one per thousand packets
const MAX_DECODE_ERRORS: u64 = 3;

#[derive(Debug)]
pub(crate) struct Analysis {
    pub codec: &'static str,
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_ms: u64,
    pub packets: u64,
    /// Sum of the sizes of all demuxed audio packets
    pub audio_bytes: u64,
    /// SHA-256 over the audio packets only, independent of tags
    pub audio_sha256: String,
    pub peaks: Peaks,
}

fn allowed_codec(format: AudioFormat, codec: AudioCodecId) -> Option<&'static str> {
    const PCM: [AudioCodecId; 6] = [
        CODEC_ID_PCM_U8,
        CODEC_ID_PCM_S16LE,
        CODEC_ID_PCM_S24LE,
        CODEC_ID_PCM_S32LE,
        CODEC_ID_PCM_F32LE,
        CODEC_ID_PCM_F64LE,
    ];

    match format {
        AudioFormat::Flac if codec == CODEC_ID_FLAC => Some("FLAC"),
        AudioFormat::Mp3 if codec == CODEC_ID_MP3 => Some("MP3"),
        AudioFormat::Mp4 if codec == CODEC_ID_AAC => Some("AAC"),
        AudioFormat::Mp4 if codec == CODEC_ID_ALAC => Some("ALAC"),
        AudioFormat::Ogg if codec == CODEC_ID_VORBIS => Some("Vorbis"),
        AudioFormat::Wav if PCM.contains(&codec) => Some("PCM"),
        _ => None,
    }
}

pub(crate) fn open_reader<'s>(
    format: AudioFormat,
    mss: MediaSourceStream<'s>,
) -> symphonia::core::errors::Result<Box<dyn FormatReader + 's>> {
    let opts = FormatOptions::default();
    // The reader is chosen from the already verified signature, never probed:
    // a file can only ever be parsed as the one format it claims to be
    Ok(match format {
        AudioFormat::Flac => Box::new(FlacReader::try_new(mss, opts)?),
        AudioFormat::Mp3 => Box::new(MpaReader::try_new(mss, opts)?),
        AudioFormat::Mp4 => Box::new(IsoMp4Reader::try_new(mss, opts)?),
        AudioFormat::Ogg => Box::new(OggReader::try_new(mss, opts)?),
        AudioFormat::Wav => Box::new(WavReader::try_new(mss, opts)?),
    })
}

fn demux_error(err: Error) -> Rejection {
    match err {
        Error::IoError(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            malformed("the file is truncated")
        }
        Error::IoError(e) => Rejection::Internal(e),
        Error::Unsupported(what) => Rejection::Unsupported(what.to_string()),
        Error::LimitError(what) => malformed(format!("limit exceeded: {what}")),
        Error::ResetRequired => Rejection::Unsupported(
            "the stream changes format midway; chained streams are not accepted".into(),
        ),
        other => malformed(other.to_string()),
    }
}

pub(crate) fn analyze(
    path: &Path,
    format: AudioFormat,
    limits: &Limits,
    deadline: Instant,
) -> Result<Analysis, Rejection> {
    let mss = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut reader = open_reader(format, mss).map_err(demux_error)?;

    let tracks = reader.tracks();
    if tracks.len() != 1 {
        return Err(Rejection::Unsupported(format!(
            "the file contains {} streams; exactly one audio stream is accepted",
            tracks.len()
        )));
    }
    let track = &tracks[0];
    let track_id = track.id;
    let params = match &track.codec_params {
        Some(CodecParameters::Audio(params)) => params.clone(),
        _ => return Err(Rejection::Unsupported("the stream is not audio".into())),
    };

    let codec = allowed_codec(format, params.codec).ok_or_else(|| {
        Rejection::Unsupported(format!(
            "this codec is not accepted in {} files",
            format.display_name()
        ))
    })?;

    let sample_rate = params
        .sample_rate
        .ok_or_else(|| malformed("the sample rate is unknown"))?;
    if !limits.sample_rates.contains(&sample_rate) {
        return Err(Rejection::Unsupported(format!(
            "a sample rate of {sample_rate} Hz is not accepted"
        )));
    }

    let channels = params.channels.as_ref().map_or(0, |c| c.count()) as u32;
    if channels == 0 || channels > limits.max_channels {
        return Err(Rejection::Unsupported(format!(
            "{channels} audio channels are not accepted"
        )));
    }

    let mut decoder_opts = AudioDecoderOptions::default();
    // Check embedded checksums (e.g. the FLAC MD5 signature) where available
    decoder_opts.verify = true;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &decoder_opts)
        .map_err(demux_error)?;

    let max_frames = limits.max_duration.as_secs() * u64::from(sample_rate);
    let mut frames = 0u64;
    let mut packets = 0u64;
    let mut audio_bytes = 0u64;
    let mut decode_errors = 0u64;
    let mut peaks = PeakAccumulator::new(sample_rate, channels as usize);
    let mut samples: Vec<f32> = Vec::new();
    let mut audio_hash = Sha256::new();

    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(err) => return Err(demux_error(err)),
        };

        if packet.track_id != track_id {
            return Err(Rejection::Unsupported(
                "the file contains more than one stream".into(),
            ));
        }

        packets += 1;
        audio_bytes += packet.data.len() as u64;
        audio_hash.update(&packet.data);

        match decoder.decode(&packet) {
            Ok(decoded) => {
                frames += decoded.frames() as u64;
                if frames > max_frames {
                    return Err(Rejection::TooLong(limits.max_duration.as_secs() / 60));
                }
                decoded.copy_to_vec_interleaved(&mut samples);
                peaks.push(&samples);
            }
            Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => {
                decode_errors += 1;
                if decode_errors > MAX_DECODE_ERRORS + packets / 1000 {
                    return Err(malformed("too many corrupt audio frames"));
                }
            }
            Err(err) => return Err(demux_error(err)),
        }

        if frames > max_frames {
            return Err(Rejection::TooLong(limits.max_duration.as_secs() / 60));
        }
        if packets % 64 == 0 && Instant::now() > deadline {
            return Err(Rejection::Timeout);
        }
    }

    if decoder.finalize().verify_ok == Some(false) {
        return Err(malformed("the audio checksum does not match"));
    }
    if frames == 0 {
        return Err(malformed("the file contains no audio"));
    }

    Ok(Analysis {
        codec,
        sample_rate,
        channels,
        duration_ms: frames * 1000 / u64::from(sample_rate),
        packets,
        audio_bytes,
        audio_sha256: hex::encode(audio_hash.finalize()),
        peaks: peaks.finish(),
    })
}

/// Computes the per-channel absolute peak of each 250 ms window, matching
/// what the client's GStreamer `level` based waveform generator produces.
struct PeakAccumulator {
    channels: usize,
    window: usize,
    filled: usize,
    current: (f32, f32),
    peaks: Peaks,
}

impl PeakAccumulator {
    fn new(sample_rate: u32, channels: usize) -> Self {
        Self {
            channels,
            window: (sample_rate / PEAKS_PER_SECOND).max(1) as usize,
            filled: 0,
            current: (0.0, 0.0),
            peaks: Vec::new(),
        }
    }

    fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.channels) {
            let left = frame[0].abs();
            let right = frame.get(1).map_or(left, |s| s.abs());
            self.current.0 = self.current.0.max(left);
            self.current.1 = self.current.1.max(right);
            self.filled += 1;
            if self.filled == self.window {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        let clamp = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        self.peaks
            .push((clamp(self.current.0), clamp(self.current.1)));
        self.current = (0.0, 0.0);
        self.filled = 0;
    }

    fn finish(mut self) -> Peaks {
        if self.filled > 0 {
            self.flush();
        }
        self.peaks
    }
}
